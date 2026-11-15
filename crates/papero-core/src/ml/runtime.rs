use crate::config::bundled_resource_dirs;
use crate::error::{CoreError, Result};
use ort::environment::Environment;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static ORT_ENV: OnceLock<(Environment, PathBuf)> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// Discovers an ONNX Runtime shared library across explicit config, `ORT_DYLIB_PATH`,
/// bundled app resources, and standard OS package paths.
pub fn resolved_onnxruntime_path(explicit: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = explicit {
        candidates.push(p.to_path_buf());
    }
    if let Ok(env_p) = std::env::var("ORT_DYLIB_PATH") {
        let trimmed = env_p.trim();
        if !trimmed.is_empty() {
            candidates.push(PathBuf::from(trimmed));
        }
    }

    #[cfg(target_os = "macos")]
    let lib_name = "libonnxruntime.dylib";
    #[cfg(target_os = "windows")]
    let lib_name = "onnxruntime.dll";
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let lib_name = "libonnxruntime.so";

    for dir in bundled_resource_dirs() {
        candidates.push(dir.join(lib_name));
    }

    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from("/opt/homebrew/opt/onnxruntime/lib").join(lib_name));
        candidates.push(PathBuf::from("/opt/homebrew/lib").join(lib_name));
        candidates.push(PathBuf::from("/usr/local/opt/onnxruntime/lib").join(lib_name));
        candidates.push(PathBuf::from("/usr/local/lib").join(lib_name));
    }

    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/usr/local/lib").join(lib_name));
        candidates.push(PathBuf::from("/usr/lib").join(lib_name));
        candidates.push(PathBuf::from("/usr/lib/x86_64-linux-gnu").join(lib_name));
        candidates.push(PathBuf::from("/usr/lib/aarch64-linux-gnu").join(lib_name));
    }

    candidates.into_iter().find(|p| p.exists() && p.is_file())
}

/// Returns the process-wide ONNX Runtime [`Environment`], dynamically loading
/// `libonnxruntime` / `onnxruntime.dll` on first use.
pub fn ensure_ort_environment(explicit_path: Option<&Path>) -> Result<(Environment, PathBuf)> {
    if let Some((env, path)) = ORT_ENV.get() {
        return Ok((env.clone(), path.clone()));
    }
    let _guard = INIT_LOCK
        .lock()
        .map_err(|_| CoreError::Config("ORT init mutex poisoned".into()))?;
    if let Some((env, path)) = ORT_ENV.get() {
        return Ok((env.clone(), path.clone()));
    }

    let lib_path = resolved_onnxruntime_path(explicit_path).ok_or_else(|| {
        CoreError::Config(
            "ONNX Runtime shared library not found. Install onnxruntime or configure onnxruntime_lib_path / ORT_DYLIB_PATH.".to_string(),
        )
    })?;

    let env = ort::init_from(&lib_path)
        .map_err(|e| CoreError::Config(format!("failed loading ORT {}: {e}", lib_path.display())))?
        .with_name("papero")
        .build()
        .map_err(|e| CoreError::Config(format!("failed building ORT environment: {e}")))?;

    let _ = ORT_ENV.set((env.clone(), lib_path.clone()));
    Ok((env, lib_path))
}
