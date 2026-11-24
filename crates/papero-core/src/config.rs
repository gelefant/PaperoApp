use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
pub use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Application data directory layout.
/// Derived files (cache, logs, indices, extracted metadata) always live here,
/// never inside the user's source document folders.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub tantivy_dir: PathBuf,
    pub indices_dir: PathBuf,
    pub models_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub config_path: PathBuf,
}

impl AppPaths {
    pub fn from_data_dir(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        Self {
            db_path: data_dir.join("papero.db"),
            tantivy_dir: data_dir.join("tantivy"),
            indices_dir: data_dir.join("indices"),
            models_dir: data_dir.join("models"),
            cache_dir: data_dir.join("cache"),
            logs_dir: data_dir.join("logs"),
            config_path: data_dir.join("config.json"),
            data_dir,
        }
    }

    /// Returns the standard OS-specific application data directory for `org.papero.app`.
    pub fn default_app_dir() -> PathBuf {
        if let Ok(override_dir) = std::env::var("PAPERO_DATA_DIR") {
            if !override_dir.trim().is_empty() {
                return PathBuf::from(override_dir);
            }
        }
        #[cfg(target_os = "macos")]
        {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home)
                    .join("Library")
                    .join("Application Support")
                    .join("org.papero.app");
            }
        }
        #[cfg(target_os = "windows")]
        {
            if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
                return select_windows_app_dir(Path::new(&local_app_data));
            }
        }
        #[cfg(target_os = "linux")]
        {
            if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
                if !xdg.trim().is_empty() {
                    return PathBuf::from(xdg).join("papero");
                }
            }
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home)
                    .join(".local")
                    .join("share")
                    .join("papero");
            }
        }
        PathBuf::from(".papero-data")
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [
            &self.data_dir,
            &self.tantivy_dir,
            &self.indices_dir,
            &self.models_dir,
            &self.cache_dir,
            &self.logs_dir,
        ] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Local-only JSON mapping `root_id -> native root_path` for this device.
    /// Never exported inside `.papero-backup`, enabling automatic cross-OS folder rebinding.
    pub fn local_roots_path(&self) -> PathBuf {
        self.data_dir.join("local_root_paths.json")
    }

    /// Directory holding a verified staged `.papero-backup` payload applied on next launch.
    pub fn pending_restore_dir(&self) -> PathBuf {
        self.data_dir.join(".pending_restore")
    }
}

/// Default ordered list of bibliographic metadata sources in the resolution waterfall.
pub const DEFAULT_METADATA_SOURCE_PRIORITY: &[&str] = &[
    "mrlookup",
    "zbmath",
    "crossref",
    "dblp",
    "openalex",
    "inspirehep",
    "arxiv",
    "datacite",
    "semanticscholar",
    "europepmc",
    "harvard",
    "loc",
    "googlebooks",
    "openlibrary",
];

/// Normalizes a user-supplied source priority slice so every known source appears
/// exactly once: preserving valid user-ordered IDs first and inserting any newly
/// added book catalogs (`harvard`, `loc`, `googlebooks`) right above `openlibrary`
/// when migrating older configs.
pub fn normalize_metadata_source_priority(raw: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(DEFAULT_METADATA_SOURCE_PRIORITY.len());
    for item in raw {
        let key = item.trim().to_ascii_lowercase();
        if let Some(&canon) = DEFAULT_METADATA_SOURCE_PRIORITY
            .iter()
            .find(|&&s| s == key)
        {
            if !out.iter().any(|existing| existing == canon) {
                out.push(canon.to_string());
            }
        }
    }
    for &canon in DEFAULT_METADATA_SOURCE_PRIORITY {
        if !out.iter().any(|existing| existing == canon) {
            if matches!(canon, "harvard" | "loc" | "googlebooks") {
                if let Some(ol_pos) = out.iter().position(|s| s == "openlibrary") {
                    out.insert(ol_pos, canon.to_string());
                    continue;
                }
            }
            out.push(canon.to_string());
        }
    }
    out
}

/// How "Show in Folder" behaves on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LinuxRevealMode {
    #[default]
    DbusWithFallback,
    ParentXdgOpenOnly,
}

/// Persistent user configuration (contains NO API keys or secrets).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Maximum worker threads for background extraction and indexing.
    pub max_worker_threads: usize,
    /// Optional path to `libpdfium` dynamic library.
    pub pdfium_lib_path: Option<PathBuf>,
    /// Optional path to `djvused` executable.
    pub djvused_bin: Option<String>,
    /// Optional path to `ddjvu` executable.
    pub ddjvu_bin: Option<String>,
    /// Target tokens per passage chunk (default 400, within 300-500).
    pub chunk_target_tokens: usize,
    /// Overlap tokens between consecutive chunks on the same page.
    pub chunk_overlap_tokens: usize,
    /// Maximum disk cache size in MB for rendered pages / temporary files.
    pub cache_limit_mb: u64,
    /// Whether clicking/opening a document opens the internal viewer in a separate window (false = tabbed).
    pub viewer_open_in_separate_window: bool,
    /// External PDF command template with `{file}` and `{page}` placeholders (None = OS default).
    pub external_pdf_command: Option<String>,
    /// External DjVu command template with `{file}` and `{page}` placeholders (None = OS default).
    pub external_djvu_command: Option<String>,
    /// Linux file manager reveal behavior.
    pub linux_reveal_mode: LinuxRevealMode,
    /// Optional fallback for polite email (`mailto`) ONLY when OS keyring is unavailable.
    pub polite_email_fallback: Option<String>,
    /// Master switch to allow/disable all online bibliographic metadata lookups (offline mode when false).
    pub network_lookups_enabled: bool,
    /// Allow user-initiated single-record MRLookup queries (default true).
    pub mrlookup_enabled_single: bool,
    /// Allow bulk automated MRLookup queries during rescan (default false per AMS ToS).
    pub mrlookup_enabled_bulk: bool,
    /// Enable zbMATH Open metadata lookup.
    pub zbmath_enabled: bool,
    /// Enable Crossref metadata lookup.
    pub crossref_enabled: bool,
    /// Enable arXiv Atom metadata lookup.
    pub arxiv_enabled: bool,
    /// Enable Harvard LibraryCloud academic book catalog lookup.
    pub harvard_enabled: bool,
    /// Enable Library of Congress SRU book catalog lookup.
    pub loc_enabled: bool,
    /// Enable Google Books metadata lookup.
    pub googlebooks_enabled: bool,
    /// Enable Open Library single-book fallback lookup.
    pub openlibrary_enabled: bool,
    /// Enable DBLP Computer Science Bibliography metadata lookup.
    pub dblp_enabled: bool,
    /// Enable OpenAlex metadata lookup.
    pub openalex_enabled: bool,
    /// Enable DataCite metadata lookup.
    pub datacite_enabled: bool,
    /// Enable INSPIRE-HEP metadata lookup.
    pub inspirehep_enabled: bool,
    /// Enable Europe PMC metadata lookup.
    pub europepmc_enabled: bool,
    /// Enable Semantic Scholar Graph API metadata lookup.
    pub semanticscholar_enabled: bool,
    /// Ordered priority list of metadata source IDs queried during waterfall resolution.
    pub metadata_source_priority: Vec<String>,
    /// Active ML profile: `"light"` (default, 384-d) or `"quality"` (1024-d).
    pub ml_profile: String,
    /// Optional explicit path to dynamic `libonnxruntime` / `onnxruntime.dll` shared library.
    pub onnxruntime_lib_path: Option<PathBuf>,
    /// Whether to run cross-encoder reranking after RRF (default true).
    pub reranker_enabled: bool,
    /// Seconds of inactivity before unloading ONNX models to reclaim idle RAM (default 180s).
    pub model_idle_unload_secs: u64,
    /// Top-N candidates retrieved from each branch (BM25 and Vector) before RRF (default 50).
    pub hybrid_top_n: usize,
    /// Top-M RRF candidates sent to cross-encoder reranker on Light profile (default 25).
    pub rerank_top_m_light: usize,
    /// Top-M RRF candidates sent to cross-encoder reranker on Quality profile (default 12 on CPU).
    pub rerank_top_m_quality: usize,
    /// Maximum returned documents (0 = unlimited above min_score, default 30).
    pub search_top_k_docs: usize,
    /// Minimum normalized score threshold applied after aggregation (default 0.0).
    pub search_min_score: f32,
    /// Document score aggregation method: `"max"` (default) or `"top2_mean"`.
    pub doc_score_aggregation: String,
    /// UI language code (`"it"` or `"en"`).
    pub ui_language: String,
    /// UI theme preference (`"system"`, `"light"`, or `"dark"`).
    pub ui_theme: String,
    /// Whether the 4-step First-Run Wizard has been completed or skipped.
    pub first_run_completed: bool,
    /// Whether real-time filesystem monitoring is active on enabled source folders.
    pub fs_watcher_enabled: bool,
    /// Optional explicit path to external `tesseract` executable.
    pub tesseract_bin: Option<String>,
    /// Optional Tesseract language expression (e.g. `"eng+ita+fra"`). `None` auto-selects installed packs.
    pub ocr_languages: Option<String>,
    /// Rasterization DPI for OCR page rendering (`150..=350`, default `250`).
    pub ocr_dpi: u32,
    /// Optional generative LLM provider (`disabled`, `ollama`, `anthropic`, `openai`, `gemini`).
    pub llm_provider: crate::llm::LlmProvider,
    /// Selected model ID fetched from the active LLM provider endpoint.
    pub llm_model: Option<String>,
    /// Local Ollama server base URL (default `http://127.0.0.1:11434`).
    pub llm_ollama_url: String,
    /// Maximum retrieved passages sent per LLM answer request.
    pub llm_max_passages: usize,
    /// Maximum estimated input tokens sent per LLM answer request.
    pub llm_max_input_tokens: usize,
    /// Maximum output tokens requested from the LLM provider.
    pub llm_max_output_tokens: usize,
    /// Whether the user accepted the first-send privacy notice for Anthropic API.
    pub llm_consent_anthropic: bool,
    /// Whether the user accepted the first-send privacy notice for OpenAI API.
    pub llm_consent_openai: bool,
    /// Whether the user accepted the first-send privacy notice (incl. Free Tier warning) for Google Gemini API.
    pub llm_consent_gemini: bool,
    /// Optional path to the user's primary `.papero-backup` file for automatic updates.
    pub backup_path: Option<String>,
    /// Whether Papero should automatically refresh `backup_path` after library changes once created.
    pub auto_backup_enabled: bool,
    /// Unix timestamp (seconds) of the last completed backup.
    pub last_backup_at: Option<i64>,
    /// Payload size in bytes of the last completed backup.
    pub last_backup_bytes: Option<u64>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        Self {
            max_worker_threads: (cpus / 2).max(1),
            pdfium_lib_path: None,
            djvused_bin: None,
            ddjvu_bin: None,
            chunk_target_tokens: 400,
            chunk_overlap_tokens: 60,
            cache_limit_mb: 500,
            viewer_open_in_separate_window: false,
            external_pdf_command: None,
            external_djvu_command: None,
            linux_reveal_mode: LinuxRevealMode::DbusWithFallback,
            polite_email_fallback: None,
            network_lookups_enabled: true,
            mrlookup_enabled_single: true,
            mrlookup_enabled_bulk: false,
            zbmath_enabled: true,
            crossref_enabled: true,
            arxiv_enabled: true,
            harvard_enabled: true,
            loc_enabled: true,
            googlebooks_enabled: true,
            openlibrary_enabled: true,
            dblp_enabled: true,
            openalex_enabled: true,
            datacite_enabled: true,
            inspirehep_enabled: true,
            europepmc_enabled: true,
            semanticscholar_enabled: true,
            metadata_source_priority: DEFAULT_METADATA_SOURCE_PRIORITY
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            ml_profile: "light".to_string(),
            onnxruntime_lib_path: None,
            reranker_enabled: true,
            model_idle_unload_secs: 180,
            hybrid_top_n: 50,
            rerank_top_m_light: 25,
            rerank_top_m_quality: 12,
            search_top_k_docs: 30,
            search_min_score: 0.0,
            doc_score_aggregation: "max".to_string(),
            ui_language: "it".to_string(),
            ui_theme: "system".to_string(),
            first_run_completed: false,
            fs_watcher_enabled: true,
            tesseract_bin: None,
            ocr_languages: None,
            ocr_dpi: 250,
            llm_provider: crate::llm::LlmProvider::Disabled,
            llm_model: None,
            llm_ollama_url: "http://127.0.0.1:11434".to_string(),
            llm_max_passages: 8,
            llm_max_input_tokens: 3500,
            llm_max_output_tokens: 1024,
            llm_consent_anthropic: false,
            llm_consent_openai: false,
            llm_consent_gemini: false,
            backup_path: None,
            auto_backup_enabled: true,
            last_backup_at: None,
            last_backup_bytes: None,
        }
    }
}

/// Returns candidate directories where bundled native libraries and external CLI tools
/// (PDFium, ONNX Runtime, DjVuLibre binaries) may be located in dev or packaged bundles.
pub fn bundled_resource_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            dirs.push(parent.to_path_buf());
            dirs.push(parent.join("resources"));
            #[cfg(target_os = "macos")]
            {
                dirs.push(parent.join("../Resources"));
                dirs.push(parent.join("../Resources/resources"));
                dirs.push(parent.join("../Frameworks"));
            }
            #[cfg(target_os = "linux")]
            {
                // Standard `.deb` / AppImage resource layout (`/usr/lib/<pkg>/resources` etc.)
                dirs.push(parent.join("../lib/Papero/resources"));
                dirs.push(parent.join("../lib/papero/resources"));
                dirs.push(parent.join("../lib/papero-app/resources"));
                dirs.push(parent.join("../share/Papero/resources"));
                dirs.push(parent.join("../share/papero/resources"));
            }
        }
    }
    dirs.push(PathBuf::from("src-tauri/resources"));
    dirs
}

/// Resolves an external CLI executable (`djvused`, `ddjvu`, `tesseract`, ...) across:
/// 1. Explicit user/config path,
/// 2. Bundled application resource directories,
/// 3. Standard Homebrew/system binary paths on macOS (where Finder `.app` launches omit Homebrew from `PATH`),
/// 4. Plain executable name resolved via OS `PATH`.
pub fn resolve_external_tool(explicit: Option<&str>, tool_name: &str) -> String {
    if let Some(p) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return p.to_string();
    }

    #[cfg(target_os = "windows")]
    let bin_name = if tool_name.to_ascii_lowercase().ends_with(".exe") {
        tool_name.to_string()
    } else {
        format!("{tool_name}.exe")
    };
    #[cfg(not(target_os = "windows"))]
    let bin_name = tool_name.to_string();

    for dir in bundled_resource_dirs() {
        let candidate = dir.join(&bin_name);
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    #[cfg(target_os = "macos")]
    {
        for prefix in ["/opt/homebrew/bin", "/usr/local/bin"] {
            let candidate = Path::new(prefix).join(&bin_name);
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    tool_name.to_string()
}

/// Configures a background CLI `Command` so it does not flash a transient console window on Windows.
pub fn configure_background_command(
    cmd: &mut std::process::Command,
) -> &mut std::process::Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

impl AppConfig {
    pub fn load_or_default(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)?;
        if raw.trim().is_empty() {
            tracing::warn!(path = %path.display(), "config.json is empty; falling back to defaults");
            return Ok(Self::default());
        }
        match serde_json::from_str(&raw) {
            Ok(cfg) => Ok(cfg),
            Err(err) => {
                let backup_path = path.with_extension("json.corrupt");
                let _ = fs::copy(path, &backup_path);
                tracing::warn!(
                    path = %path.display(),
                    backup = %backup_path.display(),
                    error = %err,
                    "failed to parse config.json; backed up corrupt file and restored defaults"
                );
                Ok(Self::default())
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }
}

/// Strips host-specific targets and secret fallbacks before exporting a portable backup.
pub fn sanitize_config_for_backup(cfg: &mut AppConfig) {
    cfg.polite_email_fallback = None;
    cfg.backup_path = None;
    cfg.last_backup_at = None;
    cfg.last_backup_bytes = None;
}

fn is_incompatible_tool_path(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return true;
    }
    if cfg!(not(target_os = "windows"))
        && (trimmed.to_ascii_lowercase().ends_with(".exe") || trimmed.contains('\\'))
    {
        return true;
    }
    (trimmed.contains('/') || trimmed.contains('\\')) && !Path::new(trimmed).exists()
}

fn is_incompatible_external_command(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    let macos_only = trimmed == "macos-preview-applescript"
        || trimmed.starts_with("/Applications/")
        || lower.contains(".app/");
    if cfg!(not(target_os = "macos")) && macos_only {
        return true;
    }
    if cfg!(not(target_os = "windows")) && lower.contains(".exe") {
        return true;
    }
    cfg!(target_os = "windows") && trimmed.starts_with('/')
}

/// Sanitizes an imported `AppConfig` so backup files remain portable across macOS, Windows, and Linux.
pub fn sanitize_config_for_cross_os_restore(cfg: &mut AppConfig) {
    sanitize_config_for_backup(cfg);

    if cfg.pdfium_lib_path.as_ref().is_some_and(|p| !p.exists()) {
        cfg.pdfium_lib_path = None;
    }
    if cfg
        .onnxruntime_lib_path
        .as_ref()
        .is_some_and(|p| !p.exists())
    {
        cfg.onnxruntime_lib_path = None;
    }
    for slot in [
        &mut cfg.djvused_bin,
        &mut cfg.ddjvu_bin,
        &mut cfg.tesseract_bin,
    ] {
        if slot.as_deref().is_some_and(is_incompatible_tool_path) {
            *slot = None;
        }
    }
    for slot in [
        &mut cfg.external_pdf_command,
        &mut cfg.external_djvu_command,
    ] {
        if slot.as_deref().is_some_and(is_incompatible_external_command) {
            *slot = None;
        }
    }
}

/// Initializes structured file logging to `<AppData>/logs/papero.log` (daily rolling).
/// Keep the returned `WorkerGuard` alive for the duration of the process.
pub fn init_logging(logs_dir: &Path) -> Result<WorkerGuard> {
    fs::create_dir_all(logs_dir)?;
    let file_appender = tracing_appender::rolling::daily(logs_dir, "papero.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("papero_core=info,papero_app=info,warn"));

    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().with_writer(non_blocking).with_ansi(false))
        .try_init();

    Ok(guard)
}

/// Resolves the Windows application data directory inside `%LOCALAPPDATA%`.
/// Uses `%LOCALAPPDATA%\org.papero.app` so user databases/indices never collide with Tauri NSIS's
/// default `currentUser` binary installation directory (`%LOCALAPPDATA%\Papero`), while preserving
/// standalone dev data in `%LOCALAPPDATA%\Papero` when no NSIS installation lives there.
pub fn select_windows_app_dir(local_app_data: &Path) -> PathBuf {
    let preferred = local_app_data.join("org.papero.app");
    if preferred.join("papero.db").exists() || preferred.join("config.json").exists() {
        return preferred;
    }
    let legacy = local_app_data.join("Papero");
    if legacy.join("papero.db").exists()
        && !legacy.join("uninstall.exe").exists()
        && !legacy.join("papero-app.exe").exists()
    {
        return legacy;
    }
    preferred
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_app_dir_avoids_nsis_install_dir_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        // Fresh install defaults to `%LOCALAPPDATA%/org.papero.app`
        assert_eq!(select_windows_app_dir(base), base.join("org.papero.app"));

        // Standalone dev directory with `papero.db` and no NSIS binaries stays on legacy path
        let legacy = base.join("Papero");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("papero.db"), b"sqlite").unwrap();
        assert_eq!(select_windows_app_dir(base), legacy);

        // If NSIS binaries (`uninstall.exe` / `papero-app.exe`) live in `%LOCALAPPDATA%/Papero`,
        // `org.papero.app` is chosen so upgrades/uninstalls never mix binaries with user data.
        fs::write(legacy.join("uninstall.exe"), b"MZ").unwrap();
        assert_eq!(select_windows_app_dir(base), base.join("org.papero.app"));
    }

    #[test]
    fn load_or_default_recovers_from_empty_or_corrupt_config_json() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.json");

        fs::write(&cfg_path, "   \n").unwrap();
        let cfg = AppConfig::load_or_default(&cfg_path).unwrap();
        assert_eq!(cfg.ml_profile, "light");

        fs::write(&cfg_path, "{corrupt_json").unwrap();
        let cfg2 = AppConfig::load_or_default(&cfg_path).unwrap();
        assert_eq!(cfg2.ml_profile, "light");
        assert!(tmp.path().join("config.json.corrupt").exists());
    }

    #[test]
    fn normalize_source_priority_places_harvard_loc_googlebooks_above_openlibrary() {
        let legacy: Vec<String> = vec![
            "mrlookup",
            "zbmath",
            "crossref",
            "dblp",
            "openalex",
            "inspirehep",
            "arxiv",
            "datacite",
            "semanticscholar",
            "europepmc",
            "openlibrary",
        ]
        .into_iter()
        .map(String::from)
        .collect();

        let migrated = normalize_metadata_source_priority(&legacy);
        let pos = |id: &str| migrated.iter().position(|s| s == id).unwrap();
        assert!(pos("harvard") < pos("loc"));
        assert!(pos("loc") < pos("googlebooks"));
        assert!(pos("googlebooks") < pos("openlibrary"));
        assert_eq!(pos("openlibrary"), migrated.len() - 1);
    }
}
