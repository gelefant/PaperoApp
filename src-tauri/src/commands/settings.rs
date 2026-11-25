use crate::commands::search::spawn_background_embedder;
use crate::state::AppState;
use papero_core::backup::{stage_pending_restore, BackupReport, RestoreReport};
use papero_core::bibtex::{BibtexFormat, BibtexKeyPattern};
use papero_core::config::{normalize_metadata_source_priority, AppConfig, LinuxRevealMode};
use papero_core::credentials::{load_polite_email, save_polite_email};
use papero_core::extract::ocr::{
    detect_tesseract, enqueue_ocr_for_scanned_files, run_ocr_for_file, run_pending_ocr_queue,
    OcrEnvironmentStatus, OcrFileReport, OcrProgressEvent,
};
use papero_core::index::hybrid::DocScoreAggregation;
use papero_core::llm::LlmProvider;
use papero_core::maintenance::{
    check_github_latest_release, clear_render_cache, compact_database, compute_storage_health,
    CompactionReport, StorageHealthReport, UpdateCheckResult, DEFAULT_GITHUB_REPO_SLUG,
};
use papero_core::metadata::resolver::WaterfallClients;
use papero_core::ml::ModelProfile;
use papero_core::viewer::os_actions::open_with_os_default;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};

fn default_true_setting() -> bool {
    true
}

fn default_ocr_front_pages_setting() -> u32 {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSettingsPayload {
    pub viewer_open_in_separate_window: bool,
    pub external_pdf_command: Option<String>,
    pub external_djvu_command: Option<String>,
    pub linux_reveal_mode: LinuxRevealMode,
    pub max_worker_threads: usize,
    pub cache_limit_mb: u64,
    pub polite_email: Option<String>,
    pub network_lookups_enabled: bool,
    pub mrlookup_enabled_single: bool,
    pub mrlookup_enabled_bulk: bool,
    pub zbmath_enabled: bool,
    pub crossref_enabled: bool,
    pub arxiv_enabled: bool,
    #[serde(default = "default_true_setting")]
    pub harvard_enabled: bool,
    #[serde(default = "default_true_setting")]
    pub loc_enabled: bool,
    #[serde(default = "default_true_setting")]
    pub googlebooks_enabled: bool,
    pub openlibrary_enabled: bool,
    pub dblp_enabled: bool,
    pub openalex_enabled: bool,
    pub datacite_enabled: bool,
    pub inspirehep_enabled: bool,
    pub europepmc_enabled: bool,
    pub semanticscholar_enabled: bool,
    #[serde(default)]
    pub metadata_source_priority: Vec<String>,
    pub ml_profile: String,
    pub reranker_enabled: bool,
    pub model_idle_unload_secs: u64,
    pub hybrid_top_n: usize,
    pub rerank_top_m_light: usize,
    pub rerank_top_m_quality: usize,
    pub search_top_k_docs: usize,
    pub search_min_score: f32,
    pub doc_score_aggregation: String,
    pub ui_language: String,
    pub ui_theme: String,
    pub first_run_completed: bool,
    pub fs_watcher_enabled: bool,
    pub tesseract_bin: Option<String>,
    pub ocr_languages: Option<String>,
    pub ocr_dpi: u32,
    #[serde(default)]
    pub ocr_front_pages_only: bool,
    #[serde(default = "default_ocr_front_pages_setting")]
    pub ocr_front_pages_count: u32,
    #[serde(default = "default_true_setting")]
    pub check_updates_on_startup: bool,
    pub llm_provider: LlmProvider,
    pub llm_model: Option<String>,
    pub llm_ollama_url: String,
    pub llm_max_passages: usize,
    pub llm_max_input_tokens: usize,
    pub llm_max_output_tokens: usize,
    pub llm_consent_anthropic: bool,
    pub llm_consent_openai: bool,
    pub llm_consent_gemini: bool,
    #[serde(default)]
    pub backup_path: Option<String>,
    #[serde(default = "default_true_setting")]
    pub auto_backup_enabled: bool,
    #[serde(default)]
    pub last_backup_at: Option<i64>,
    #[serde(default)]
    pub last_backup_bytes: Option<u64>,
    #[serde(default)]
    pub bibtex_key_pattern: BibtexKeyPattern,
    #[serde(default)]
    pub bibtex_format: BibtexFormat,
    #[serde(default)]
    pub bibtex_auto_sync_path: Option<String>,
    #[serde(default)]
    pub bibtex_auto_sync_enabled: bool,
}

#[tauri::command]
pub async fn get_ui_settings(state: State<'_, Arc<AppState>>) -> Result<UiSettingsPayload, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let cfg = st.config.read().expect("config poisoned").clone();
        let polite_email = load_polite_email(&cfg);
        UiSettingsPayload {
            viewer_open_in_separate_window: cfg.viewer_open_in_separate_window,
            external_pdf_command: cfg.external_pdf_command.clone(),
            external_djvu_command: cfg.external_djvu_command.clone(),
            linux_reveal_mode: cfg.linux_reveal_mode,
            max_worker_threads: cfg.max_worker_threads,
            cache_limit_mb: cfg.cache_limit_mb,
            polite_email,
            network_lookups_enabled: cfg.network_lookups_enabled,
            mrlookup_enabled_single: cfg.mrlookup_enabled_single,
            mrlookup_enabled_bulk: cfg.mrlookup_enabled_bulk,
            zbmath_enabled: cfg.zbmath_enabled,
            crossref_enabled: cfg.crossref_enabled,
            arxiv_enabled: cfg.arxiv_enabled,
            harvard_enabled: cfg.harvard_enabled,
            loc_enabled: cfg.loc_enabled,
            googlebooks_enabled: cfg.googlebooks_enabled,
            openlibrary_enabled: cfg.openlibrary_enabled,
            dblp_enabled: cfg.dblp_enabled,
            openalex_enabled: cfg.openalex_enabled,
            datacite_enabled: cfg.datacite_enabled,
            inspirehep_enabled: cfg.inspirehep_enabled,
            europepmc_enabled: cfg.europepmc_enabled,
            semanticscholar_enabled: cfg.semanticscholar_enabled,
            metadata_source_priority: normalize_metadata_source_priority(
                &cfg.metadata_source_priority,
            ),
            ml_profile: cfg.ml_profile.clone(),
            reranker_enabled: cfg.reranker_enabled,
            model_idle_unload_secs: cfg.model_idle_unload_secs,
            hybrid_top_n: cfg.hybrid_top_n,
            rerank_top_m_light: cfg.rerank_top_m_light,
            rerank_top_m_quality: cfg.rerank_top_m_quality,
            search_top_k_docs: cfg.search_top_k_docs,
            search_min_score: cfg.search_min_score,
            doc_score_aggregation: cfg.doc_score_aggregation.clone(),
            ui_language: cfg.ui_language.clone(),
            ui_theme: cfg.ui_theme.clone(),
            first_run_completed: cfg.first_run_completed,
            fs_watcher_enabled: cfg.fs_watcher_enabled,
            tesseract_bin: cfg.tesseract_bin.clone(),
            ocr_languages: cfg.ocr_languages.clone(),
            ocr_dpi: cfg.ocr_dpi,
            ocr_front_pages_only: cfg.ocr_front_pages_only,
            ocr_front_pages_count: cfg.ocr_front_pages_count,
            check_updates_on_startup: cfg.check_updates_on_startup,
            llm_provider: cfg.llm_provider,
            llm_model: cfg.llm_model.clone(),
            llm_ollama_url: cfg.llm_ollama_url.clone(),
            llm_max_passages: cfg.llm_max_passages,
            llm_max_input_tokens: cfg.llm_max_input_tokens,
            llm_max_output_tokens: cfg.llm_max_output_tokens,
            llm_consent_anthropic: cfg.llm_consent_anthropic,
            llm_consent_openai: cfg.llm_consent_openai,
            llm_consent_gemini: cfg.llm_consent_gemini,
            backup_path: cfg.backup_path.clone(),
            auto_backup_enabled: cfg.auto_backup_enabled,
            last_backup_at: cfg.last_backup_at,
            last_backup_bytes: cfg.last_backup_bytes,
            bibtex_key_pattern: cfg.bibtex_key_pattern,
            bibtex_format: cfg.bibtex_format,
            bibtex_auto_sync_path: cfg.bibtex_auto_sync_path.clone(),
            bibtex_auto_sync_enabled: cfg.bibtex_auto_sync_enabled,
        }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_ui_settings(
    state: State<'_, Arc<AppState>>,
    settings: UiSettingsPayload,
) -> Result<(), String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut cfg: AppConfig = st.config.read().expect("config poisoned").clone();
        let prev_profile = cfg.ml_profile.clone();
        cfg.viewer_open_in_separate_window = settings.viewer_open_in_separate_window;
        cfg.external_pdf_command = settings.external_pdf_command;
        cfg.external_djvu_command = settings.external_djvu_command;
        cfg.linux_reveal_mode = settings.linux_reveal_mode;
        cfg.max_worker_threads = settings.max_worker_threads.max(1);
        cfg.cache_limit_mb = settings.cache_limit_mb.max(50);
        cfg.network_lookups_enabled = settings.network_lookups_enabled;
        cfg.mrlookup_enabled_single = settings.mrlookup_enabled_single;
        cfg.mrlookup_enabled_bulk = settings.mrlookup_enabled_bulk;
        cfg.zbmath_enabled = settings.zbmath_enabled;
        cfg.crossref_enabled = settings.crossref_enabled;
        cfg.arxiv_enabled = settings.arxiv_enabled;
        cfg.harvard_enabled = settings.harvard_enabled;
        cfg.loc_enabled = settings.loc_enabled;
        cfg.googlebooks_enabled = settings.googlebooks_enabled;
        cfg.openlibrary_enabled = settings.openlibrary_enabled;
        cfg.dblp_enabled = settings.dblp_enabled;
        cfg.openalex_enabled = settings.openalex_enabled;
        cfg.datacite_enabled = settings.datacite_enabled;
        cfg.inspirehep_enabled = settings.inspirehep_enabled;
        cfg.europepmc_enabled = settings.europepmc_enabled;
        cfg.semanticscholar_enabled = settings.semanticscholar_enabled;
        cfg.metadata_source_priority =
            normalize_metadata_source_priority(&settings.metadata_source_priority);
        cfg.ml_profile = ModelProfile::from_str_lossy(&settings.ml_profile)
            .as_str()
            .to_string();
        cfg.reranker_enabled = settings.reranker_enabled;
        cfg.model_idle_unload_secs = settings.model_idle_unload_secs.clamp(30, 3600);
        cfg.hybrid_top_n = settings.hybrid_top_n.clamp(10, 200);
        cfg.rerank_top_m_light = settings.rerank_top_m_light.clamp(5, 100);
        cfg.rerank_top_m_quality = settings.rerank_top_m_quality.clamp(4, 60);
        cfg.search_top_k_docs = settings.search_top_k_docs;
        cfg.search_min_score = settings.search_min_score.clamp(0.0, 1.0);
        cfg.doc_score_aggregation =
            match DocScoreAggregation::from_str_lossy(&settings.doc_score_aggregation) {
                DocScoreAggregation::Top2Mean => "top2_mean".to_string(),
                DocScoreAggregation::Max => "max".to_string(),
            };
        cfg.ui_language = match settings.ui_language.trim().to_ascii_lowercase().as_str() {
            "en" => "en".to_string(),
            _ => "it".to_string(),
        };
        cfg.ui_theme = match settings.ui_theme.trim().to_ascii_lowercase().as_str() {
            "light" => "light".to_string(),
            "dark" => "dark".to_string(),
            _ => "system".to_string(),
        };
        cfg.first_run_completed = settings.first_run_completed;
        cfg.fs_watcher_enabled = settings.fs_watcher_enabled;
        cfg.tesseract_bin = settings
            .tesseract_bin
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        cfg.ocr_languages = settings
            .ocr_languages
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        cfg.ocr_dpi = settings.ocr_dpi.clamp(150, 350);
        cfg.ocr_front_pages_only = settings.ocr_front_pages_only;
        cfg.ocr_front_pages_count = settings.ocr_front_pages_count.clamp(1, 50);
        cfg.check_updates_on_startup = settings.check_updates_on_startup;
        cfg.llm_provider = settings.llm_provider;
        cfg.llm_model = settings
            .llm_model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let ollama_url = settings.llm_ollama_url.trim();
        cfg.llm_ollama_url = if ollama_url.is_empty() {
            LlmProvider::Ollama.default_base_url().to_string()
        } else {
            ollama_url.trim_end_matches('/').to_string()
        };
        cfg.llm_max_passages = settings.llm_max_passages.clamp(1, 40);
        cfg.llm_max_input_tokens = settings.llm_max_input_tokens.clamp(256, 64_000);
        cfg.llm_max_output_tokens = settings.llm_max_output_tokens.clamp(128, 8192);
        cfg.llm_consent_anthropic = settings.llm_consent_anthropic;
        cfg.llm_consent_openai = settings.llm_consent_openai;
        cfg.llm_consent_gemini = settings.llm_consent_gemini;
        cfg.auto_backup_enabled = settings.auto_backup_enabled;
        let next_backup_path = settings
            .backup_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        if cfg.backup_path != next_backup_path {
            cfg.backup_path = next_backup_path;
            if cfg.backup_path.is_none() {
                cfg.last_backup_at = None;
                cfg.last_backup_bytes = None;
            }
        }
        let bibtex_changed = cfg.bibtex_key_pattern != settings.bibtex_key_pattern
            || cfg.bibtex_format != settings.bibtex_format
            || cfg.bibtex_auto_sync_enabled != settings.bibtex_auto_sync_enabled
            || cfg.bibtex_auto_sync_path != settings.bibtex_auto_sync_path;
        cfg.bibtex_key_pattern = settings.bibtex_key_pattern;
        cfg.bibtex_format = settings.bibtex_format;
        cfg.bibtex_auto_sync_path = settings
            .bibtex_auto_sync_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        cfg.bibtex_auto_sync_enabled =
            settings.bibtex_auto_sync_enabled && cfg.bibtex_auto_sync_path.is_some();
        cfg.save(&st.paths.config_path).map_err(|e| e.to_string())?;

        save_polite_email(settings.polite_email.as_deref(), Some(&st.paths.config_path))
            .map_err(|e| e.to_string())?;
        let reloaded =
            AppConfig::load_or_default(&st.paths.config_path).map_err(|e| e.to_string())?;
        let clients =
            Arc::new(WaterfallClients::from_config(&reloaded).map_err(|e| e.to_string())?);

        *st.clients.write().expect("clients poisoned") = clients;
        *st.config.write().expect("config poisoned") = reloaded;

        if prev_profile != cfg.ml_profile {
            st.cancel_embed_worker();
            st.embedder_slot.unload();
            st.reranker_slot.unload();
            spawn_background_embedder(Arc::clone(&st));
        }
        AppState::refresh_watcher(&st, None);
        st.schedule_auto_backup();
        if bibtex_changed {
            st.schedule_bibtex_auto_sync();
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn create_backup_cmd(
    state: State<'_, Arc<AppState>>,
    dest_path: String,
) -> Result<BackupReport, String> {
    let trimmed = dest_path.trim();
    if trimmed.is_empty() {
        return Err("Backup destination path is empty".to_string());
    }
    let normalized = if trimmed.to_ascii_lowercase().ends_with(".papero-backup") {
        PathBuf::from(trimmed)
    } else {
        PathBuf::from(format!("{trimmed}.papero-backup"))
    };
    AppState::run_backup_and_record(&state, &normalized).await
}

#[tauri::command]
pub async fn update_backup_now_cmd(
    state: State<'_, Arc<AppState>>,
) -> Result<BackupReport, String> {
    let dest = {
        let cfg = state.config.read().expect("config poisoned");
        cfg.backup_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| {
                "Nessun percorso di backup configurato. Crea prima un backup.".to_string()
            })?
    };
    AppState::run_backup_and_record(&state, &dest).await
}

#[tauri::command]
pub async fn restore_backup_cmd(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    src_path: String,
) -> Result<RestoreReport, String> {
    let _guard = state.backup_mutex.lock().await;
    state.stop_background_activity();
    let st = Arc::clone(&state);
    let rep = tokio::task::spawn_blocking(move || {
        stage_pending_restore(&st.paths, Path::new(&src_path)).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        app.restart();
    });
    Ok(rep)
}

#[tauri::command]
pub async fn get_ocr_status(
    state: State<'_, Arc<AppState>>,
) -> Result<OcrEnvironmentStatus, String> {
    let (bin, langs) = {
        let cfg = state.config.read().expect("config poisoned");
        (cfg.tesseract_bin.clone(), cfg.ocr_languages.clone())
    };
    tokio::task::spawn_blocking(move || detect_tesseract(bin.as_deref(), langs.as_deref()))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn run_ocr_for_file_cmd(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    on_event: Channel<OcrProgressEvent>,
) -> Result<OcrFileReport, String> {
    let st = Arc::clone(&state);
    let rep = tokio::task::spawn_blocking(move || {
        let cfg = st.config.read().expect("config poisoned").clone();
        let profile = ModelProfile::from_str_lossy(&cfg.ml_profile);
        let vindex = st.vector_index_for(profile);
        run_ocr_for_file(
            &st.db,
            &st.bm25,
            Some(vindex.as_ref()),
            &st.paths.cache_dir,
            &cfg,
            file_id,
            None,
            |ev| {
                let _ = on_event.send(ev);
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    spawn_background_embedder(Arc::clone(&state));
    state.schedule_auto_backup();
    Ok(rep)
}

#[tauri::command]
pub async fn run_pending_ocr_cmd(
    state: State<'_, Arc<AppState>>,
    on_event: Channel<OcrProgressEvent>,
) -> Result<Vec<OcrFileReport>, String> {
    let st = Arc::clone(&state);
    let reports = tokio::task::spawn_blocking(move || {
        let cfg = st.config.read().expect("config poisoned").clone();
        let profile = ModelProfile::from_str_lossy(&cfg.ml_profile);
        let vindex = st.vector_index_for(profile);
        {
            let w = st.db.writer();
            let _ = enqueue_ocr_for_scanned_files(&w);
        }
        run_pending_ocr_queue(
            &st.db,
            &st.bm25,
            Some(vindex.as_ref()),
            &st.paths.cache_dir,
            &cfg,
            None,
            |ev| {
                let _ = on_event.send(ev);
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    if !reports.is_empty() {
        spawn_background_embedder(Arc::clone(&state));
        state.schedule_auto_backup();
    }
    Ok(reports)
}

#[tauri::command]
pub async fn get_storage_health_cmd(
    state: State<'_, Arc<AppState>>,
) -> Result<StorageHealthReport, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        compute_storage_health(&st.paths, &st.db).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn compact_database_cmd(
    state: State<'_, Arc<AppState>>,
) -> Result<CompactionReport, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        compact_database(&st.paths, &st.db).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clear_render_cache_cmd(state: State<'_, Arc<AppState>>) -> Result<u64, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || clear_render_cache(&st.paths).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn check_app_update_cmd(
    state: State<'_, Arc<AppState>>,
    manual: Option<bool>,
) -> Result<UpdateCheckResult, String> {
    let cfg = state.config.read().expect("config poisoned").clone();
    let current = env!("CARGO_PKG_VERSION");
    if !cfg.network_lookups_enabled && !manual.unwrap_or(false) {
        return Ok(UpdateCheckResult {
            current_version: current.to_string(),
            latest_version: current.to_string(),
            update_available: false,
            release_url: format!("https://github.com/{DEFAULT_GITHUB_REPO_SLUG}/releases"),
            release_notes: None,
            published_at: None,
        });
    }
    check_github_latest_release(current, DEFAULT_GITHUB_REPO_SLUG)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_external_url_cmd(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if (!trimmed.starts_with("https://") && !trimmed.starts_with("http://"))
        || trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(
            "Solo link http:// o https:// validi possono essere aperti nel browser.".to_string(),
        );
    }
    let u = trimmed.to_string();
    tokio::task::spawn_blocking(move || open_with_os_default(Path::new(&u)).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}
