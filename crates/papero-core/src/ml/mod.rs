pub mod embedder;
pub mod registry;
pub mod reranker;
pub mod runtime;
pub mod slot;
pub mod worker;

pub use embedder::OnnxEmbedder;
pub use registry::{
    download_model_profile, list_model_statuses, model_dir_for, model_specs_for_profile,
    profile_is_ready, sha256_file_hex, DownloadProgressEvent, ModelArtifact, ModelProfile,
    ModelProfileStatus, ModelRole, ModelSpec,
};
pub use reranker::OnnxReranker;
pub use runtime::{ensure_ort_environment, resolved_onnxruntime_path};
pub use slot::{current_process_rss_bytes, LazyModelSlot};
pub use worker::run_ml_worker_stdio;
