use crate::error::{CoreError, Result};
use crate::ml::embedder::{build_cpu_or_coreml_session, encodings_to_arrays};
use crate::ml::registry::{model_dir_for, model_specs_for_profile, ModelProfile, ModelRole, ModelSpec};
use crate::ml::runtime::ensure_ort_environment;
use crate::ml::worker::MlWorkerClient;
use ndarray::IxDyn;
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;
use tokenizers::{Encoding, PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

pub const DEFAULT_RERANK_MAX_SEQ_LEN: usize = 384;
pub const DEFAULT_RERANK_BATCH_SIZE: usize = 4;

enum RerankerBackend {
    InProcess {
        tokenizer: Box<Tokenizer>,
        session: Session,
        needs_token_type_ids: bool,
        batch_size: usize,
    },
    Worker(MlWorkerClient),
}

pub struct OnnxReranker {
    spec: ModelSpec,
    max_seq_len: usize,
    backend: RerankerBackend,
}

impl OnnxReranker {
    /// Loads the cross-encoder ONNX reranker from `<models_dir>/<profile>/reranker/`.
    pub fn load(
        models_dir: &Path,
        profile: ModelProfile,
        ort_lib_path: Option<&Path>,
        intra_threads: usize,
    ) -> Result<Self> {
        let spec = model_specs_for_profile(profile)
            .into_iter()
            .find(|s| s.role == ModelRole::Reranker)
            .ok_or_else(|| CoreError::Config("reranker spec missing".into()))?;
        let dir = model_dir_for(models_dir, profile, ModelRole::Reranker);
        let tokenizer_path = dir.join("tokenizer.json");
        let onnx_rel = spec
            .artifacts
            .iter()
            .find(|a| a.rel_path.ends_with(".onnx"))
            .map(|a| a.rel_path.as_str())
            .ok_or_else(|| CoreError::Config("reranker onnx artifact missing".into()))?;
        let onnx_path = dir.join(onnx_rel);
        if !tokenizer_path.exists() || !onnx_path.exists() {
            return Err(CoreError::Config(format!(
                "Reranker model '{}' not downloaded in {}",
                spec.id,
                dir.display()
            )));
        }

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| CoreError::Config(format!("failed loading reranker tokenizer: {e}")))?;
        let max_seq_len = spec.max_seq_len.min(DEFAULT_RERANK_MAX_SEQ_LEN);
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: max_seq_len,
                ..Default::default()
            }))
            .map_err(|e| CoreError::Config(format!("failed configuring reranker truncation: {e}")))?;
        tokenizer.with_padding(None);

        let (env, _) = ensure_ort_environment(ort_lib_path)?;
        let session = build_cpu_or_coreml_session(&env, &onnx_path, intra_threads.max(1))?;
        let needs_token_type_ids = session
            .inputs()
            .iter()
            .any(|o| o.name() == "token_type_ids");

        Ok(Self {
            spec,
            max_seq_len,
            backend: RerankerBackend::InProcess {
                tokenizer: Box::new(tokenizer),
                session,
                needs_token_type_ids,
                batch_size: DEFAULT_RERANK_BATCH_SIZE,
            },
        })
    }

    /// Loads the cross-encoder ONNX reranker inside an isolated `--ml-worker` child process.
    pub fn load_isolated(
        models_dir: &Path,
        profile: ModelProfile,
        ort_lib_path: Option<&Path>,
        intra_threads: usize,
    ) -> Result<Self> {
        let spec = model_specs_for_profile(profile)
            .into_iter()
            .find(|s| s.role == ModelRole::Reranker)
            .ok_or_else(|| CoreError::Config("reranker spec missing".into()))?;
        let max_seq_len = spec.max_seq_len.min(DEFAULT_RERANK_MAX_SEQ_LEN);
        let client = MlWorkerClient::spawn(
            ModelRole::Reranker,
            models_dir,
            profile,
            ort_lib_path,
            intra_threads,
        )?;
        Ok(Self {
            spec,
            max_seq_len,
            backend: RerankerBackend::Worker(client),
        })
    }

    pub fn spec(&self) -> &ModelSpec {
        &self.spec
    }

    pub fn max_seq_len(&self) -> usize {
        self.max_seq_len
    }

    /// Scores `(query, passage)` pairs using length-sorted mini-batches of 4 truncated
    /// to `max_seq_len = 384`, returning sigmoid probabilities in `[0, 1]` aligned with input order.
    pub fn score_pairs(&mut self, query: &str, passages: &[&str]) -> Result<Vec<f32>> {
        if passages.is_empty() {
            return Ok(Vec::new());
        }
        let (encodings, batches) = match &mut self.backend {
            RerankerBackend::Worker(w) => return w.score_pairs(query, passages),
            RerankerBackend::InProcess {
                tokenizer,
                batch_size,
                ..
            } => {
                let pairs: Vec<(&str, &str)> = passages.iter().map(|p| (query, *p)).collect();
                let encodings = tokenizer
                    .encode_batch(pairs, true)
                    .map_err(|e| CoreError::Config(format!("reranker tokenization failed: {e}")))?;
                let lengths: Vec<usize> = encodings.iter().map(|e| e.len()).collect();
                let batches = length_sorted_batches(&lengths, *batch_size);
                (encodings, batches)
            }
        };

        let mut scores = vec![0.0_f32; passages.len()];
        let pad_params = PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        };

        for batch_indices in batches {
            let max_in_batch = batch_indices
                .iter()
                .map(|&idx| encodings[idx].len())
                .max()
                .unwrap_or(1)
                .max(1);
            let mut batch_encs: Vec<Encoding> = batch_indices
                .iter()
                .map(|&idx| {
                    let mut e = encodings[idx].clone();
                    e.pad(
                        max_in_batch,
                        pad_params.pad_id,
                        pad_params.pad_type_id,
                        &pad_params.pad_token,
                        tokenizers::PaddingDirection::Right,
                    );
                    e
                })
                .collect();

            let logits = self.run_logit_batch(&mut batch_encs)?;
            for (local_i, &orig_idx) in batch_indices.iter().enumerate() {
                scores[orig_idx] = sigmoid(logits[local_i]);
            }
        }

        Ok(scores)
    }

    fn run_logit_batch(&mut self, encodings: &mut [Encoding]) -> Result<Vec<f32>> {
        let RerankerBackend::InProcess {
            session,
            needs_token_type_ids,
            ..
        } = &mut self.backend
        else {
            return Err(CoreError::Config("run_logit_batch called on worker backend".into()));
        };
        let (input_ids, attention_mask, token_type_ids) = encodings_to_arrays(encodings)?;
        let outputs = if *needs_token_type_ids {
            let ids_ref = TensorRef::from_array_view(&input_ids)
                .map_err(|e| CoreError::Config(format!("ORT reranker input_ids error: {e}")))?;
            let mask_ref = TensorRef::from_array_view(&attention_mask)
                .map_err(|e| CoreError::Config(format!("ORT reranker mask error: {e}")))?;
            let type_ref = TensorRef::from_array_view(&token_type_ids)
                .map_err(|e| CoreError::Config(format!("ORT reranker token_type_ids error: {e}")))?;
            session
                .run(ort::inputs![
                    "input_ids" => ids_ref,
                    "attention_mask" => mask_ref,
                    "token_type_ids" => type_ref
                ])
                .map_err(|e| CoreError::Config(format!("ORT reranker inference failed: {e}")))?
        } else {
            let ids_ref = TensorRef::from_array_view(&input_ids)
                .map_err(|e| CoreError::Config(format!("ORT reranker input_ids error: {e}")))?;
            let mask_ref = TensorRef::from_array_view(&attention_mask)
                .map_err(|e| CoreError::Config(format!("ORT reranker mask error: {e}")))?;
            session
                .run(ort::inputs![
                    "input_ids" => ids_ref,
                    "attention_mask" => mask_ref
                ])
                .map_err(|e| CoreError::Config(format!("ORT reranker inference failed: {e}")))?
        };

        let (_, first_val) = outputs
            .iter()
            .find(|(name, _)| *name == "logits")
            .or_else(|| outputs.iter().next())
            .ok_or_else(|| CoreError::Config("reranker returned no output tensors".into()))?;

        let arr = first_val
            .try_extract_array::<f32>()
            .map_err(|e| CoreError::Config(format!("failed extracting reranker logits: {e}")))?;
        extract_logits(arr.view())
    }
}

pub(crate) fn length_sorted_batches(lengths: &[usize], batch_size: usize) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..lengths.len()).collect();
    order.sort_by_key(|&idx| lengths[idx]);
    order
        .chunks(batch_size.max(1))
        .map(|chunk| chunk.to_vec())
        .collect()
}

pub(crate) fn extract_logits(tensor: ndarray::ArrayView<'_, f32, IxDyn>) -> Result<Vec<f32>> {
    let shape = tensor.shape();
    match shape.len() {
        1 => Ok((0..shape[0]).map(|b| tensor[[b]]).collect()),
        2 => {
            let batch = shape[0];
            let classes = shape[1];
            let mut out = Vec::with_capacity(batch);
            for b in 0..batch {
                let val = if classes == 1 {
                    tensor[[b, 0]]
                } else {
                    tensor[[b, 1]] - tensor[[b, 0]]
                };
                out.push(val);
            }
            Ok(out)
        }
        other => Err(CoreError::Config(format!(
            "unexpected reranker output rank {other}"
        ))),
    }
}

pub fn sigmoid(x: f32) -> f32 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let ex = x.exp();
        ex / (1.0 + ex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_sorted_batches_groups_similar_lengths() {
        let lengths = [300, 12, 180, 14, 290, 10];
        let batches = length_sorted_batches(&lengths, 3);
        assert_eq!(batches.len(), 2);
        // Short group indices: 5 (10), 1 (12), 3 (14)
        assert_eq!(batches[0], vec![5, 1, 3]);
        // Long group indices: 2 (180), 4 (290), 0 (300)
        assert_eq!(batches[1], vec![2, 4, 0]);
    }

    #[test]
    fn sigmoid_monotonic_and_bounded() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
        assert!(sigmoid(8.0) > 0.999);
        assert!(sigmoid(-8.0) < 0.001);
        assert!(sigmoid(100.0).is_finite());
        assert!(sigmoid(-100.0).is_finite());
    }
}
