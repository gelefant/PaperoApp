use crate::error::{CoreError, Result};
use futures_util::StreamExt;
use reqwest::header::RANGE;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelProfile {
    Light,
    Quality,
}

impl ModelProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelProfile::Light => "light",
            ModelProfile::Quality => "quality",
        }
    }

    pub fn from_str_lossy(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "quality" | "bge-m3" => ModelProfile::Quality,
            _ => ModelProfile::Light,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelRole {
    Embedder,
    Reranker,
}

impl ModelRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelRole::Embedder => "embedder",
            ModelRole::Reranker => "reranker",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelArtifact {
    pub rel_path: String,
    pub url: String,
    pub expected_bytes: u64,
    pub sha256_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    pub id: String,
    pub profile: ModelProfile,
    pub role: ModelRole,
    pub hf_repo: String,
    pub license: String,
    pub embedding_dim: usize,
    pub max_seq_len: usize,
    pub uses_e5_prefixes: bool,
    pub uses_cls_pooling: bool,
    pub artifacts: Vec<ModelArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfileStatus {
    pub profile: ModelProfile,
    pub label: String,
    pub ready: bool,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub embedder_repo: String,
    pub reranker_repo: String,
    pub missing_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgressEvent {
    pub profile: ModelProfile,
    pub artifact_name: String,
    pub artifact_bytes_done: u64,
    pub artifact_bytes_total: u64,
    pub profile_bytes_done: u64,
    pub profile_bytes_total: u64,
    pub bytes_per_sec: f64,
    pub finished: bool,
}

pub fn model_specs_for_profile(profile: ModelProfile) -> Vec<ModelSpec> {
    match profile {
        ModelProfile::Light => vec![light_embedder_spec(), light_reranker_spec()],
        ModelProfile::Quality => vec![quality_embedder_spec(), quality_reranker_spec()],
    }
}

fn light_embedder_spec() -> ModelSpec {
    let repo = "Xenova/multilingual-e5-small";
    ModelSpec {
        id: "light-embedder".to_string(),
        profile: ModelProfile::Light,
        role: ModelRole::Embedder,
        hf_repo: repo.to_string(),
        license: "MIT".to_string(),
        embedding_dim: 384,
        max_seq_len: 512,
        uses_e5_prefixes: true,
        uses_cls_pooling: false,
        artifacts: vec![
            ModelArtifact {
                rel_path: "config.json".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/config.json"),
                expected_bytes: 658,
                sha256_hex: "cb99455288675345e1a4f411438d5d0adbba5fbd3a67ea4fb03c015433b996c1"
                    .to_string(),
            },
            ModelArtifact {
                rel_path: "tokenizer.json".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/tokenizer.json"),
                expected_bytes: 17_082_730,
                sha256_hex: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39"
                    .to_string(),
            },
            ModelArtifact {
                rel_path: "onnx/model_quantized.onnx".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/onnx/model_quantized.onnx"),
                expected_bytes: 118_308_185,
                sha256_hex: "f80102d3f2a1229f387d3c81909990d8945513e347b0eab049f7de3c6f98c193"
                    .to_string(),
            },
        ],
    }
}

fn light_reranker_spec() -> ModelSpec {
    let repo = "cross-encoder/mmarco-mMiniLMv2-L12-H384-v1";
    #[cfg(target_arch = "aarch64")]
    let (onnx_remote, onnx_bytes, onnx_sha) = (
        "onnx/model_qint8_arm64.onnx",
        118_620_017u64,
        "1825907d6c1a9001ff78124780bbde20a614a8c3df3b63409cf3c72c6fe5c8b4",
    );
    #[cfg(not(target_arch = "aarch64"))]
    let (onnx_remote, onnx_bytes, onnx_sha) = (
        "onnx/model_quint8_avx2.onnx",
        118_620_016u64,
        "6c2513767fb63d008a4377bef7a7a3555433d9436342bb53e35a3a72ffc52d4b",
    );

    ModelSpec {
        id: "light-reranker".to_string(),
        profile: ModelProfile::Light,
        role: ModelRole::Reranker,
        hf_repo: repo.to_string(),
        license: "Apache-2.0".to_string(),
        embedding_dim: 384,
        max_seq_len: 384,
        uses_e5_prefixes: false,
        uses_cls_pooling: false,
        artifacts: vec![
            ModelArtifact {
                rel_path: "config.json".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/config.json"),
                expected_bytes: 891,
                sha256_hex: "cc2cfe51aa3fd759d21d21acf5dfd6994aa67a3c9210636d22e143699d336c77"
                    .to_string(),
            },
            ModelArtifact {
                rel_path: "tokenizer.json".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/tokenizer.json"),
                expected_bytes: 17_082_660,
                sha256_hex: "62c24cdc13d4c9952d63718d6c9fa4c287974249e16b7ade6d5a85e7bbb75626"
                    .to_string(),
            },
            ModelArtifact {
                rel_path: "onnx/model_quantized.onnx".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/{onnx_remote}"),
                expected_bytes: onnx_bytes,
                sha256_hex: onnx_sha.to_string(),
            },
        ],
    }
}

fn quality_embedder_spec() -> ModelSpec {
    let repo = "Xenova/bge-m3";
    ModelSpec {
        id: "quality-embedder".to_string(),
        profile: ModelProfile::Quality,
        role: ModelRole::Embedder,
        hf_repo: repo.to_string(),
        license: "MIT".to_string(),
        embedding_dim: 1024,
        max_seq_len: 512,
        uses_e5_prefixes: false,
        uses_cls_pooling: true,
        artifacts: vec![
            ModelArtifact {
                rel_path: "config.json".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/config.json"),
                expected_bytes: 770,
                sha256_hex: "734a79bf12d388c1467a4e3ab625f45de7f6906cffcfb93a1eca1787504bed95"
                    .to_string(),
            },
            ModelArtifact {
                rel_path: "tokenizer.json".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/tokenizer.json"),
                expected_bytes: 17_082_821,
                sha256_hex: "6710678b12670bc442b99edc952c4d996ae309a7020c1fa0096dd245c2faf790"
                    .to_string(),
            },
            ModelArtifact {
                rel_path: "onnx/model_quantized.onnx".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/onnx/model_quantized.onnx"),
                expected_bytes: 569_694_530,
                sha256_hex: "0826f8c1ab9edf1801db86c61919d4d108e8bfc0b809ec823ad366882ff0b77d"
                    .to_string(),
            },
        ],
    }
}

fn quality_reranker_spec() -> ModelSpec {
    let repo = "onnx-community/bge-reranker-v2-m3-ONNX";
    ModelSpec {
        id: "quality-reranker".to_string(),
        profile: ModelProfile::Quality,
        role: ModelRole::Reranker,
        hf_repo: repo.to_string(),
        license: "Apache-2.0".to_string(),
        embedding_dim: 1024,
        max_seq_len: 384,
        uses_e5_prefixes: false,
        uses_cls_pooling: true,
        artifacts: vec![
            ModelArtifact {
                rel_path: "config.json".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/config.json"),
                expected_bytes: 848,
                sha256_hex: "122e922dcfed6503c8721e6fe1daf090340c3d95ca7f3aa3a72730b321a51cfd"
                    .to_string(),
            },
            ModelArtifact {
                rel_path: "tokenizer.json".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/tokenizer.json"),
                expected_bytes: 17_082_900,
                sha256_hex: "8bf8afbfd11306bd872018c53bfdf2e160a56f8edbcf49933324404791c148d3"
                    .to_string(),
            },
            ModelArtifact {
                rel_path: "onnx/model_quantized.onnx".to_string(),
                url: format!("https://huggingface.co/{repo}/resolve/main/onnx/model_quantized.onnx"),
                expected_bytes: 570_727_094,
                sha256_hex: "912fc1215c2dbff6499700534bd8d31253af01573861abbfc43afd1fab6cce5d"
                    .to_string(),
            },
        ],
    }
}

pub fn model_dir_for(models_dir: &Path, profile: ModelProfile, role: ModelRole) -> PathBuf {
    models_dir.join(profile.as_str()).join(role.as_str())
}

pub fn sha256_file_hex(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn is_artifact_valid(models_dir: &Path, spec: &ModelSpec, art: &ModelArtifact) -> bool {
    let path = model_dir_for(models_dir, spec.profile, spec.role).join(&art.rel_path);
    let Ok(meta) = fs::metadata(&path) else {
        return false;
    };
    if meta.len() != art.expected_bytes {
        return false;
    }
    sha256_file_hex(&path)
        .map(|d| d.eq_ignore_ascii_case(&art.sha256_hex))
        .unwrap_or(false)
}

pub fn profile_is_ready(models_dir: &Path, profile: ModelProfile) -> bool {
    model_specs_for_profile(profile)
        .iter()
        .all(|spec| spec.artifacts.iter().all(|a| is_artifact_valid(models_dir, spec, a)))
}

pub fn list_model_statuses(models_dir: &Path) -> Vec<ModelProfileStatus> {
    [ModelProfile::Light, ModelProfile::Quality]
        .into_iter()
        .map(|profile| {
            let specs = model_specs_for_profile(profile);
            let mut total_bytes = 0u64;
            let mut downloaded_bytes = 0u64;
            let mut missing_files = Vec::new();
            for spec in &specs {
                for art in &spec.artifacts {
                    total_bytes += art.expected_bytes;
                    let target = model_dir_for(models_dir, spec.profile, spec.role).join(&art.rel_path);
                    let part = part_path_for(&target);
                    if target.exists() && fs::metadata(&target).map(|m| m.len() == art.expected_bytes).unwrap_or(false) {
                        downloaded_bytes += art.expected_bytes;
                    } else {
                        if let Ok(m) = fs::metadata(&part) {
                            downloaded_bytes += m.len().min(art.expected_bytes);
                        }
                        missing_files.push(format!("{}/{}", spec.role.as_str(), art.rel_path));
                    }
                }
            }
            let label = match profile {
                ModelProfile::Light => "Light (multilingual-e5-small + mMiniLMv2 INT8, ~270 MB)".to_string(),
                ModelProfile::Quality => "Quality (BGE-M3 + bge-reranker-v2-m3 INT8, ~1.17 GB)".to_string(),
            };
            ModelProfileStatus {
                profile,
                label,
                ready: missing_files.is_empty(),
                total_bytes,
                downloaded_bytes,
                embedder_repo: specs[0].hf_repo.clone(),
                reranker_repo: specs[1].hf_repo.clone(),
                missing_files,
            }
        })
        .collect()
}

fn part_path_for(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("artifact");
    target.with_file_name(format!("{name}.part"))
}

/// Verifies a `.part` file's SHA-256 digest and atomically swaps it into `target_path`.
pub fn verify_and_swap_part(part_path: &Path, target_path: &Path, expected_sha256: &str) -> Result<()> {
    let actual_sha = sha256_file_hex(part_path)?;
    if !actual_sha.eq_ignore_ascii_case(expected_sha256) {
        let _ = fs::remove_file(part_path);
        return Err(CoreError::Network(format!(
            "SHA-256 mismatch for {}: expected {}, got {}",
            target_path.display(),
            expected_sha256,
            actual_sha
        )));
    }
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(part_path, target_path)?;
    Ok(())
}

/// Downloads all artifacts of a model profile with HTTP `Range` resume, progress callbacks,
/// and strict SHA-256 verification before atomic file placement.
pub async fn download_model_profile<F>(
    models_dir: &Path,
    profile: ModelProfile,
    on_progress: F,
) -> Result<()>
where
    F: Fn(DownloadProgressEvent) + Send + Sync,
{
    let specs = model_specs_for_profile(profile);
    let profile_total: u64 = specs
        .iter()
        .flat_map(|s| s.artifacts.iter())
        .map(|a| a.expected_bytes)
        .sum();

    let client = reqwest::Client::builder()
        .user_agent(concat!("Papero/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| CoreError::Network(format!("model downloader client init: {e}")))?;

    let mut completed_prior_bytes = 0u64;

    for spec in &specs {
        let base_dir = model_dir_for(models_dir, spec.profile, spec.role);
        for art in &spec.artifacts {
            let target = base_dir.join(&art.rel_path);
            if target.exists() && is_artifact_valid(models_dir, spec, art) {
                completed_prior_bytes += art.expected_bytes;
                on_progress(DownloadProgressEvent {
                    profile,
                    artifact_name: format!("{}/{}", spec.role.as_str(), art.rel_path),
                    artifact_bytes_done: art.expected_bytes,
                    artifact_bytes_total: art.expected_bytes,
                    profile_bytes_done: completed_prior_bytes,
                    profile_bytes_total: profile_total,
                    bytes_per_sec: 0.0,
                    finished: false,
                });
                continue;
            }

            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let part = part_path_for(&target);
            download_single_artifact_with_resume(
                &client,
                art,
                &part,
                &target,
                |done, total, speed| {
                    on_progress(DownloadProgressEvent {
                        profile,
                        artifact_name: format!("{}/{}", spec.role.as_str(), art.rel_path),
                        artifact_bytes_done: done,
                        artifact_bytes_total: total,
                        profile_bytes_done: completed_prior_bytes + done,
                        profile_bytes_total: profile_total,
                        bytes_per_sec: speed,
                        finished: false,
                    });
                },
            )
            .await?;
            completed_prior_bytes += art.expected_bytes;
        }
    }

    on_progress(DownloadProgressEvent {
        profile,
        artifact_name: "done".to_string(),
        artifact_bytes_done: 0,
        artifact_bytes_total: 0,
        profile_bytes_done: profile_total,
        profile_bytes_total: profile_total,
        bytes_per_sec: 0.0,
        finished: true,
    });
    Ok(())
}

async fn download_single_artifact_with_resume<F>(
    client: &reqwest::Client,
    art: &ModelArtifact,
    part_path: &Path,
    target_path: &Path,
    on_chunk: F,
) -> Result<()>
where
    F: Fn(u64, u64, f64),
{
    let existing_len = fs::metadata(part_path).map(|m| m.len()).unwrap_or(0);
    if existing_len > art.expected_bytes {
        let _ = fs::remove_file(part_path);
    }
    let resume_from = fs::metadata(part_path).map(|m| m.len()).unwrap_or(0);

    let mut req = client.get(&art.url);
    if resume_from > 0 && resume_from < art.expected_bytes {
        req = req.header(RANGE, format!("bytes={resume_from}-"));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| CoreError::Network(format!("download {}: {e}", art.url)))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(CoreError::Network(format!(
            "HTTP {} downloading {}",
            status.as_u16(),
            art.url
        )));
    }

    let (mut file, mut written) = if status.as_u16() == 206 && resume_from > 0 {
        let mut f = OpenOptions::new().write(true).open(part_path)?;
        f.seek(SeekFrom::End(0))?;
        (f, resume_from)
    } else {
        (File::create(part_path)?, 0u64)
    };

    let t0 = Instant::now();
    let mut session_bytes = 0u64;
    let mut stream = resp.bytes_stream();
    let mut last_emit = Instant::now();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| CoreError::Network(format!("stream read: {e}")))?;
        file.write_all(&chunk)?;
        written += chunk.len() as u64;
        session_bytes += chunk.len() as u64;

        if last_emit.elapsed().as_millis() >= 200 {
            let secs = t0.elapsed().as_secs_f64().max(0.001);
            on_chunk(written, art.expected_bytes, session_bytes as f64 / secs);
            last_emit = Instant::now();
        }
    }
    file.flush()?;
    drop(file);

    let secs = t0.elapsed().as_secs_f64().max(0.001);
    on_chunk(written, art.expected_bytes, session_bytes as f64 / secs);

    verify_and_swap_part(part_path, target_path, &art.sha256_hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn registry_verifies_sha256_and_rejects_corrupt_part() {
        let dir = tempdir().unwrap();
        let part = dir.path().join("model.onnx.part");
        let target = dir.path().join("model.onnx");
        fs::write(&part, b"papero-test-weights").unwrap();
        let expected = hex::encode(Sha256::digest(b"papero-test-weights"));

        // Wrong checksum must fail AND delete corrupt .part
        assert!(verify_and_swap_part(&part, &target, "deadbeef").is_err());
        assert!(!part.exists());
        assert!(!target.exists());

        // Matching checksum must atomically move .part -> target
        fs::write(&part, b"papero-test-weights").unwrap();
        verify_and_swap_part(&part, &target, &expected).unwrap();
        assert!(!part.exists());
        assert_eq!(fs::read(&target).unwrap(), b"papero-test-weights");
    }

    #[test]
    fn model_specs_have_valid_64_hex_sha256() {
        for p in [ModelProfile::Light, ModelProfile::Quality] {
            let specs = model_specs_for_profile(p);
            assert_eq!(specs.len(), 2);
            for s in specs {
                for a in s.artifacts {
                    assert_eq!(a.sha256_hex.len(), 64, "bad sha len for {}", a.rel_path);
                    assert!(a.expected_bytes > 0);
                }
            }
        }
    }
}
