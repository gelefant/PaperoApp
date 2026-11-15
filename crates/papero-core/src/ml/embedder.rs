use crate::error::{CoreError, Result};
use crate::ml::registry::{model_dir_for, model_specs_for_profile, ModelProfile, ModelRole, ModelSpec};
use crate::ml::runtime::ensure_ort_environment;
use crate::ml::worker::MlWorkerClient;
use ndarray::{Array2, IxDyn};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;
use tokenizers::{Encoding, PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

enum EmbedderBackend {
    InProcess {
        tokenizer: Box<Tokenizer>,
        session: Session,
        needs_token_type_ids: bool,
    },
    Worker(MlWorkerClient),
}

pub struct OnnxEmbedder {
    spec: ModelSpec,
    backend: EmbedderBackend,
}

impl OnnxEmbedder {
    /// Loads the ONNX embedder (`Light` or `Quality`) from `<models_dir>/<profile>/embedder/`.
    pub fn load(
        models_dir: &Path,
        profile: ModelProfile,
        ort_lib_path: Option<&Path>,
        intra_threads: usize,
    ) -> Result<Self> {
        let spec = model_specs_for_profile(profile)
            .into_iter()
            .find(|s| s.role == ModelRole::Embedder)
            .ok_or_else(|| CoreError::Config("embedder spec missing".into()))?;
        let dir = model_dir_for(models_dir, profile, ModelRole::Embedder);
        let tokenizer_path = dir.join("tokenizer.json");
        let onnx_rel = spec
            .artifacts
            .iter()
            .find(|a| a.rel_path.ends_with(".onnx"))
            .map(|a| a.rel_path.as_str())
            .ok_or_else(|| CoreError::Config("embedder onnx artifact missing".into()))?;
        let onnx_path = dir.join(onnx_rel);
        if !tokenizer_path.exists() || !onnx_path.exists() {
            return Err(CoreError::Config(format!(
                "Embedder model '{}' not downloaded in {}",
                spec.id,
                dir.display()
            )));
        }

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| CoreError::Config(format!("failed loading embedder tokenizer: {e}")))?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: spec.max_seq_len,
                ..Default::default()
            }))
            .map_err(|e| CoreError::Config(format!("failed configuring tokenizer truncation: {e}")))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        }));

        let (env, _) = ensure_ort_environment(ort_lib_path)?;
        let session = build_cpu_or_coreml_session(&env, &onnx_path, intra_threads.max(1))?;
        let needs_token_type_ids = session
            .inputs()
            .iter()
            .any(|o| o.name() == "token_type_ids");

        Ok(Self {
            spec,
            backend: EmbedderBackend::InProcess {
                tokenizer: Box::new(tokenizer),
                session,
                needs_token_type_ids,
            },
        })
    }

    /// Loads the ONNX embedder inside an isolated `--ml-worker` child process so unloading
    /// on idle timeout frees 100% of ONNX Runtime weights and C++ heap back to the OS.
    pub fn load_isolated(
        models_dir: &Path,
        profile: ModelProfile,
        ort_lib_path: Option<&Path>,
        intra_threads: usize,
    ) -> Result<Self> {
        let spec = model_specs_for_profile(profile)
            .into_iter()
            .find(|s| s.role == ModelRole::Embedder)
            .ok_or_else(|| CoreError::Config("embedder spec missing".into()))?;
        let client = MlWorkerClient::spawn(
            ModelRole::Embedder,
            models_dir,
            profile,
            ort_lib_path,
            intra_threads,
        )?;
        Ok(Self {
            spec,
            backend: EmbedderBackend::Worker(client),
        })
    }

    pub fn spec(&self) -> &ModelSpec {
        &self.spec
    }

    pub fn embedding_dim(&self) -> usize {
        self.spec.embedding_dim
    }

    pub fn embed_query(&mut self, query: &str) -> Result<Vec<f32>> {
        match &mut self.backend {
            EmbedderBackend::Worker(w) => w.embed_query(query),
            EmbedderBackend::InProcess { .. } => {
                let prepared = if self.spec.uses_e5_prefixes {
                    format!("query: {}", query.trim())
                } else {
                    query.trim().to_string()
                };
                let mut out = self.embed_raw_batch(&[prepared])?;
                Ok(out.pop().unwrap_or_default())
            }
        }
    }

    pub fn embed_passages(&mut self, passages: &[&str]) -> Result<Vec<Vec<f32>>> {
        if passages.is_empty() {
            return Ok(Vec::new());
        }
        match &mut self.backend {
            EmbedderBackend::Worker(w) => w.embed_passages(passages),
            EmbedderBackend::InProcess { .. } => {
                let prepared: Vec<String> = passages
                    .iter()
                    .map(|text| {
                        let trimmed = text.trim();
                        if self.spec.uses_e5_prefixes {
                            format!("passage: {trimmed}")
                        } else {
                            trimmed.to_string()
                        }
                    })
                    .collect();
                self.embed_raw_batch(&prepared)
            }
        }
    }

    fn embed_raw_batch(&mut self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        const EMBED_BATCH_SIZE: usize = 4;
        let mut all_embeddings = Vec::with_capacity(inputs.len());
        for chunk in inputs.chunks(EMBED_BATCH_SIZE) {
            let chunk_embs = self.embed_chunk(chunk)?;
            all_embeddings.extend(chunk_embs);
        }
        Ok(all_embeddings)
    }

    fn embed_chunk(&mut self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let uses_cls = self.spec.uses_cls_pooling;
        let EmbedderBackend::InProcess {
            tokenizer,
            session,
            needs_token_type_ids,
        } = &mut self.backend
        else {
            return Err(CoreError::Config("embed_chunk called on worker backend".into()));
        };
        let refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
        let encodings = tokenizer
            .encode_batch(refs, true)
            .map_err(|e| CoreError::Config(format!("embedder tokenization failed: {e}")))?;

        let (input_ids, attention_mask, token_type_ids) = encodings_to_arrays(&encodings)?;
        let mask_clone = attention_mask.clone();

        let outputs = if *needs_token_type_ids {
            let ids_ref = TensorRef::from_array_view(&input_ids)
                .map_err(|e| CoreError::Config(format!("ORT input_ids tensor error: {e}")))?;
            let mask_ref = TensorRef::from_array_view(&attention_mask)
                .map_err(|e| CoreError::Config(format!("ORT attention_mask tensor error: {e}")))?;
            let type_ref = TensorRef::from_array_view(&token_type_ids)
                .map_err(|e| CoreError::Config(format!("ORT token_type_ids tensor error: {e}")))?;
            session
                .run(ort::inputs![
                    "input_ids" => ids_ref,
                    "attention_mask" => mask_ref,
                    "token_type_ids" => type_ref
                ])
                .map_err(|e| CoreError::Config(format!("ORT embedder inference failed: {e}")))?
        } else {
            let ids_ref = TensorRef::from_array_view(&input_ids)
                .map_err(|e| CoreError::Config(format!("ORT input_ids tensor error: {e}")))?;
            let mask_ref = TensorRef::from_array_view(&attention_mask)
                .map_err(|e| CoreError::Config(format!("ORT attention_mask tensor error: {e}")))?;
            session
                .run(ort::inputs![
                    "input_ids" => ids_ref,
                    "attention_mask" => mask_ref
                ])
                .map_err(|e| CoreError::Config(format!("ORT embedder inference failed: {e}")))?
        };

        let (_, first_val) = outputs
            .iter()
            .find(|(name, _)| *name == "last_hidden_state" || *name == "sentence_embedding")
            .or_else(|| outputs.iter().next())
            .ok_or_else(|| CoreError::Config("embedder returned no output tensors".into()))?;

        let arr = first_val
            .try_extract_array::<f32>()
            .map_err(|e| CoreError::Config(format!("failed extracting f32 embedding tensor: {e}")))?;

        pool_and_normalize(arr.view(), &mask_clone, uses_cls)
    }
}

pub(crate) fn build_cpu_or_coreml_session(
    env: &ort::environment::Environment,
    onnx_path: &Path,
    intra_threads: usize,
) -> Result<Session> {
    // Use pure CPU execution provider with arena allocator & memory pattern disabled:
    // - CoreML EP recompiles & leaks shape-specific plans on dynamic transformer inputs (~1.9 GB).
    // - Disabling CPU memory arena & memory pattern avoids retaining multi-hundred-MB transient buffers.
    Session::builder(env)
        .map_err(|e| CoreError::Config(format!("ORT Session::builder failed: {e}")))?
        .with_no_environment_execution_providers()
        .map_err(|e| CoreError::Config(format!("ORT no-env-ep failed: {e}")))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| CoreError::Config(format!("ORT optimization level failed: {e}")))?
        .with_intra_threads(intra_threads)
        .map_err(|e| CoreError::Config(format!("ORT intra threads failed: {e}")))?
        .with_memory_pattern(false)
        .map_err(|e| CoreError::Config(format!("ORT memory pattern failed: {e}")))?
        .with_execution_providers([ort::ep::CPU::default()
            .with_arena_allocator(false)
            .build()])
        .map_err(|e| CoreError::Config(format!("ORT CPU EP config failed: {e}")))?
        .commit_from_file(onnx_path)
        .map_err(|e| {
            CoreError::Config(format!(
                "failed loading ONNX model {}: {e}",
                onnx_path.display()
            ))
        })
}

pub(crate) fn encodings_to_arrays(
    encodings: &[Encoding],
) -> Result<(Array2<i64>, Array2<i64>, Array2<i64>)> {
    let batch = encodings.len();
    let seq_len = encodings.iter().map(|e| e.len()).max().unwrap_or(0);
    if batch == 0 || seq_len == 0 {
        return Err(CoreError::Config("empty tokenized batch".into()));
    }
    let mut ids = Array2::<i64>::zeros((batch, seq_len));
    let mut mask = Array2::<i64>::zeros((batch, seq_len));
    let mut types = Array2::<i64>::zeros((batch, seq_len));
    for (row, enc) in encodings.iter().enumerate() {
        let e_ids = enc.get_ids();
        let e_mask = enc.get_attention_mask();
        let e_types = enc.get_type_ids();
        for col in 0..e_ids.len() {
            ids[[row, col]] = i64::from(e_ids[col]);
            mask[[row, col]] = i64::from(e_mask[col]);
            types[[row, col]] = i64::from(*e_types.get(col).unwrap_or(&0));
        }
    }
    Ok((ids, mask, types))
}

pub(crate) fn pool_and_normalize(
    tensor: ndarray::ArrayView<'_, f32, IxDyn>,
    attention_mask: &Array2<i64>,
    cls_pooling: bool,
) -> Result<Vec<Vec<f32>>> {
    let shape = tensor.shape();
    match shape.len() {
        2 => {
            let batch = shape[0];
            let dim = shape[1];
            let mut out = Vec::with_capacity(batch);
            for b in 0..batch {
                let mut vec = Vec::with_capacity(dim);
                for d in 0..dim {
                    vec.push(tensor[[b, d]]);
                }
                l2_normalize(&mut vec);
                out.push(vec);
            }
            Ok(out)
        }
        3 => {
            let batch = shape[0];
            let seq_len = shape[1];
            let dim = shape[2];
            let mut out = Vec::with_capacity(batch);
            for b in 0..batch {
                let mut vec = vec![0.0f32; dim];
                if cls_pooling {
                    for d in 0..dim {
                        vec[d] = tensor[[b, 0, d]];
                    }
                } else {
                    let mut weight_sum = 0.0f32;
                    for t in 0..seq_len {
                        let w = if attention_mask[[b, t]] > 0 { 1.0 } else { 0.0 };
                        if w > 0.0 {
                            weight_sum += w;
                            for d in 0..dim {
                                vec[d] += tensor[[b, t, d]] * w;
                            }
                        }
                    }
                    let denom = weight_sum.max(1e-9);
                    for v in &mut vec {
                        *v /= denom;
                    }
                }
                l2_normalize(&mut vec);
                out.push(vec);
            }
            Ok(out)
        }
        other => Err(CoreError::Config(format!(
            "unexpected embedder output rank {other}"
        ))),
    }
}

pub fn l2_normalize(vec: &mut [f32]) {
    let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-12 {
        for x in vec {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array3;

    #[test]
    fn mean_pooling_ignores_padding_and_normalizes() {
        let mut hidden = Array3::<f32>::zeros((1, 3, 2));
        // Token 0 = [1.0, 2.0], Token 1 = [3.0, 4.0], Token 2 (pad) = [100.0, 100.0]
        hidden[[0, 0, 0]] = 1.0;
        hidden[[0, 0, 1]] = 2.0;
        hidden[[0, 1, 0]] = 3.0;
        hidden[[0, 1, 1]] = 4.0;
        hidden[[0, 2, 0]] = 100.0;
        hidden[[0, 2, 1]] = 100.0;

        let mask = ndarray::array![[1_i64, 1, 0]];
        let pooled = pool_and_normalize(hidden.into_dyn().view(), &mask, false).unwrap();
        assert_eq!(pooled.len(), 1);
        // Mean is [2.0, 3.0], norm = sqrt(13)
        let norm = (2.0_f32 * 2.0 + 3.0 * 3.0).sqrt();
        assert!((pooled[0][0] - 2.0 / norm).abs() < 1e-5);
        assert!((pooled[0][1] - 3.0 / norm).abs() < 1e-5);
    }

    #[test]
    fn cls_pooling_reads_token_zero() {
        let mut hidden = Array3::<f32>::zeros((1, 2, 2));
        hidden[[0, 0, 0]] = 3.0;
        hidden[[0, 0, 1]] = 4.0;
        hidden[[0, 1, 0]] = 9.0;
        hidden[[0, 1, 1]] = 9.0;

        let mask = ndarray::array![[1_i64, 1]];
        let pooled = pool_and_normalize(hidden.into_dyn().view(), &mask, true).unwrap();
        assert!((pooled[0][0] - 0.6).abs() < 1e-5);
        assert!((pooled[0][1] - 0.8).abs() < 1e-5);
    }
}
