use crate::state::AppState;
use papero_core::backup::{restore_backup_bundle, BackupReport, RestoreReport};
use papero_core::bibtex::{format_mrlookup_bibtex, BibtexExportOptions};
use papero_core::classify::{
    add_tag_to_document, build_msc_tree, confirm_document_msc, list_all_tags,
    list_topic_clusters as core_list_topic_clusters, propose_msc_knn, reject_proposed_msc,
    remove_document_from_cluster, remove_tag_from_document, rename_topic_cluster,
    run_topic_clustering, set_document_cluster, ClusteringReport, KnnProposeReport, MscTreeNode,
    TagSummary, TopicClusterView,
};
use papero_core::config::{normalize_metadata_source_priority, AppConfig, LinuxRevealMode};
use papero_core::credentials::{
    delete_secret_key, load_polite_email, load_secret_key, save_polite_email, save_secret_key,
    SecretApiKey,
};
use papero_core::db::repos::docs::{
    get_file_location, get_file_viewer_info, hydrate_bm25_results_filtered, list_documents_for_ui,
    ContentSearchDocResult, DocumentListItem, FileViewerInfo,
};
use papero_core::db::repos::roots::{
    insert_source_root, list_source_roots, remove_source_root_with_cleanup, set_root_enabled,
    set_root_exclude_from_cloud_llm, NewSourceRoot, SourceRoot,
};
use papero_core::db::repos::scans::ScanSummary;
use papero_core::dedup::{
    list_duplicate_groups, merge_duplicate_group, resolve_duplicate_group, run_duplicate_detection,
    DuplicateGroupView, DuplicateReport,
};
use papero_core::extract::alt_text::{import_alt_formula_markdown, AltFormulaImportReport};
use papero_core::extract::djvu::extract_djvu_single_page_spans;
use papero_core::extract::ocr::{
    detect_tesseract, enqueue_ocr_for_scanned_files, run_ocr_for_file, run_pending_ocr_queue,
    OcrEnvironmentStatus, OcrFileReport, OcrProgressEvent,
};
use papero_core::extract::types::ExtractedSpan;
use papero_core::fs::roots::new_root_id;
use papero_core::fs::scanner::{rescan_sources, ScanControl};
use papero_core::index::hybrid::{
    hybrid_search, DocScoreAggregation, HybridSearchContext, HybridSearchOptions,
    HybridSearchResponse,
};
use papero_core::index::worker::{run_embed_queue, run_extract_bm25_queue, WorkerBatchReport};
use papero_core::llm::{
    accept_cloud_consent, generate_grounded_answer, list_models, prepare_grounded_prompt,
    verify_provider_key, LlmAnswerResult, LlmKeyVerifyResult, LlmModelInfo, LlmPromptPreview,
    LlmProvider,
};
use papero_core::metadata::io::{
    export_documents_bibtex, export_documents_csv, import_bibtex, import_zotero_csl_json,
    metadata_view_to_record,
};
use papero_core::metadata::models::DocumentMetadataView;
use papero_core::metadata::resolver::{
    apply_manual_metadata_edit_with_locks, clear_metadata_jobs_for_doc, confirm_metadata_candidate,
    dismiss_metadata_candidate, get_document_metadata as core_get_document_metadata,
    import_metadata_records_into_library, is_document_metadata_verified, list_all_document_ids,
    list_documents_for_bulk_metadata, resolve_document_metadata, run_pending_metadata_jobs,
    search_documents_metadata, set_document_metadata_verified, ManualMetadataEdit,
    MetadataLookupScope, MetadataSearchFilter, ResolveOutcome, SaveAndFetchResult, WaterfallClients,
};
use papero_core::ml::{
    current_process_rss_bytes, download_model_profile, list_model_statuses as core_list_model_statuses,
    profile_is_ready, resolved_onnxruntime_path, DownloadProgressEvent, ModelProfile,
    ModelProfileStatus, OnnxEmbedder, OnnxReranker,
};
use papero_core::viewer::os_actions::{
    external_reader_presets, open_external, reveal_in_folder, ExternalReaderPreset,
};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::ipc::{Channel, Response as IpcResponse};
use tauri::{AppHandle, Emitter, State, WebviewUrl, WebviewWindowBuilder};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum ScanProgressEvent {
    Started,
    ScanDone { summary: ScanSummary },
    IndexingDone { report: WorkerBatchReport },
    Finished,
    Error { message: String },
}

fn default_true_setting() -> bool {
    true
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
}

#[derive(Debug, Clone, Serialize)]
pub struct MlEnvironmentStatus {
    pub onnxruntime_found: bool,
    pub onnxruntime_path: Option<String>,
    pub embedder_loaded: bool,
    pub reranker_loaded: bool,
    pub rss_mb: f64,
    pub active_profile: ModelProfile,
    pub profiles: Vec<ModelProfileStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportMetadataReport {
    pub total_entries: usize,
    pub matched_documents: usize,
    pub unmatched_entries: usize,
}

#[tauri::command]
pub fn list_roots(state: State<'_, Arc<AppState>>) -> Result<Vec<SourceRoot>, String> {
    let r = state.db.reader();
    list_source_roots(&r).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_source_root(
    state: State<'_, Arc<AppState>>,
    path: String,
    label: Option<String>,
    is_network_or_cloud: Option<bool>,
) -> Result<SourceRoot, String> {
    let trimmed = path.trim();
    let p = Path::new(trimmed);
    if !p.is_dir() {
        return Err(format!("Path is not a readable directory: {trimmed}"));
    }
    let canonical = std::fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let final_label = label
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            Path::new(&canonical)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Library")
                .to_string()
        });

    let root = {
        let w = state.db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: new_root_id(),
                label: final_label,
                root_path: canonical,
                is_network_or_cloud: is_network_or_cloud.unwrap_or(false),
            },
        )
        .map_err(|e| e.to_string())?
    };
    AppState::refresh_watcher(&state, None);
    state.schedule_auto_backup();
    Ok(root)
}

#[tauri::command]
pub fn toggle_source_root(
    state: State<'_, Arc<AppState>>,
    root_id: String,
    enabled: bool,
) -> Result<(), String> {
    {
        let w = state.db.writer();
        set_root_enabled(&w, &root_id, enabled).map_err(|e| e.to_string())?;
    }
    AppState::refresh_watcher(&state, None);
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub fn delete_source_root(state: State<'_, Arc<AppState>>, root_id: String) -> Result<(), String> {
    let report = {
        let w = state.db.writer();
        remove_source_root_with_cleanup(&w, &root_id).map_err(|e| e.to_string())?
    };
    if !report.deleted_passage_ids.is_empty() || !report.deleted_document_ids.is_empty() {
        let _ = state.bm25.delete_passages(&report.deleted_passage_ids);
        for doc_id in &report.deleted_document_ids {
            let _ = state.bm25.delete_document(*doc_id as u64);
        }
        let _ = state.bm25.commit();
        if !report.deleted_passage_ids.is_empty() {
            let _ = state.vector_light.remove_keys(&report.deleted_passage_ids);
            let _ = state.vector_quality.remove_keys(&report.deleted_passage_ids);
        }
    }
    AppState::refresh_watcher(&state, None);
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn trigger_rescan(
    state: State<'_, Arc<AppState>>,
    only_root_id: Option<String>,
    on_event: Channel<ScanProgressEvent>,
) -> Result<(), String> {
    let ctrl = Arc::new(ScanControl::new());
    {
        let mut slot = state.scan_control.lock().expect("scan_control poisoned");
        *slot = Some(Arc::clone(&ctrl));
    }

    let db = Arc::clone(&state.db);
    let bm25 = Arc::clone(&state.bm25);
    let cfg = state.config.read().expect("config poisoned").clone();

    let _ = on_event.send(ScanProgressEvent::Started);

    let res = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let summary = match rescan_sources(&db, Some(&bm25), only_root_id.as_deref(), Some(&ctrl)) {
            Ok(s) => s,
            Err(e) => {
                let msg = e.to_string();
                let _ = on_event.send(ScanProgressEvent::Error { message: msg.clone() });
                return Err(msg);
            }
        };
        let _ = on_event.send(ScanProgressEvent::ScanDone { summary });

        let report = match run_extract_bm25_queue(&db, &bm25, &cfg, Some(&ctrl)) {
            Ok(r) => r,
            Err(e) => {
                let msg = e.to_string();
                let _ = on_event.send(ScanProgressEvent::Error { message: msg.clone() });
                return Err(msg);
            }
        };
        let _ = on_event.send(ScanProgressEvent::IndexingDone { report });
        let _ = on_event.send(ScanProgressEvent::Finished);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?;

    let mut slot = state.scan_control.lock().expect("scan_control poisoned");
    *slot = None;

    let st_bg = Arc::clone(&state);
    let cfg_bg = state.config.read().expect("config poisoned").clone();
    let clients_bg = Arc::clone(&state.clients.read().expect("clients poisoned"));
    if res.is_ok() && cfg_bg.network_lookups_enabled {
        tauri::async_runtime::spawn(async move {
            if let Ok(cnt) = run_pending_metadata_jobs(&st_bg.db, &clients_bg, &cfg_bg, 2000).await {
                if cnt > 0 {
                    let db_dedup = Arc::clone(&st_bg.db);
                    let _ = tokio::task::spawn_blocking(move || run_duplicate_detection(&db_dedup)).await;
                    st_bg.schedule_auto_backup();
                    if let Ok(guard) = st_bg.app_handle.lock() {
                        if let Some(app) = guard.as_ref() {
                            let _ = app.emit("library-fs-updated", ());
                        }
                    }
                }
            }
        });
    }

    if res.is_ok() {
        spawn_background_embedder(Arc::clone(&state));
        state.schedule_auto_backup();
    }

    if let Err(ref msg) = res {
        return Err(msg.clone());
    }
    Ok(())
}

#[tauri::command]
pub fn control_rescan(state: State<'_, Arc<AppState>>, action: String) -> Result<(), String> {
    let slot = state.scan_control.lock().expect("scan_control poisoned");
    if let Some(ctrl) = slot.as_ref() {
        match action.as_str() {
            "pause" => ctrl.pause(),
            "resume" => ctrl.resume(),
            "cancel" => ctrl.cancel(),
            other => return Err(format!("unknown scan control action: {other}")),
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn list_documents(state: State<'_, Arc<AppState>>) -> Result<Vec<DocumentListItem>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        list_documents_for_ui(&r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn search_content_bm25(
    state: State<'_, Arc<AppState>>,
    query: String,
    root_ids: Option<Vec<String>>,
    limit: Option<usize>,
    work_category: Option<String>,
) -> Result<Vec<ContentSearchDocResult>, String> {
    let base_limit = limit.unwrap_or(50);
    let fetch_limit = match work_category.as_deref() {
        Some("book") | Some("paper") => (base_limit * 6).max(300),
        _ => base_limit,
    };
    let hits = state
        .bm25
        .search(&query, root_ids.as_deref(), fetch_limit)
        .map_err(|e| e.to_string())?;
    let r = state.db.reader();
    let mut docs =
        hydrate_bm25_results_filtered(&r, &hits, work_category.as_deref()).map_err(|e| e.to_string())?;
    if docs.len() > base_limit {
        docs.truncate(base_limit);
    }
    Ok(docs)
}

#[tauri::command]
pub async fn get_viewer_doc(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
) -> Result<FileViewerInfo, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        get_file_viewer_info(&r, file_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("file {file_id} not found"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn read_pdf_range_cmd(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    start: u64,
    length: usize,
) -> Result<IpcResponse, String> {
    let db = Arc::clone(&state.db);
    let buf = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let loc = {
            let r = db.reader();
            get_file_location(&r, file_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("file {file_id} not found"))?
        };
        if !loc.is_available || !loc.abs_path.exists() {
            return Err("Cartella sorgente non raggiungibile.".to_string());
        }
        let mut f = std::fs::File::open(&loc.abs_path).map_err(|e| e.to_string())?;
        let total = f.metadata().map_err(|e| e.to_string())?.len();
        if start >= total || length == 0 {
            return Ok(Vec::new());
        }
        let capped_len = (length as u64)
            .min(total - start)
            .min(4 * 1024 * 1024) as usize;
        let mut out = vec![0u8; capped_len];
        f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
        f.read_exact(&mut out).map_err(|e| e.to_string())?;
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(IpcResponse::new(buf))
}

#[tauri::command]
pub async fn render_djvu_page_cmd(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    page_ord: u32,
    dpi: Option<u32>,
) -> Result<IpcResponse, String> {
    let st = Arc::clone(&state);
    let png_bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let loc = {
            let r = st.db.reader();
            get_file_location(&r, file_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("file {file_id} not found"))?
        };
        if !loc.is_available || !loc.abs_path.exists() {
            return Err("Cartella sorgente non raggiungibile.".to_string());
        }
        let ddjvu_bin = st.config.read().expect("config poisoned").ddjvu_bin.clone();
        st.djvu_cache
            .render_page_png(
                &loc.abs_path,
                &loc.blake3_hash,
                page_ord.max(1),
                dpi.unwrap_or(150),
                ddjvu_bin.as_deref(),
            )
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    Ok(IpcResponse::new(png_bytes))
}

#[tauri::command]
pub async fn get_djvu_page_text(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    page_ord: u32,
) -> Result<Vec<ExtractedSpan>, String> {
    let loc = {
        let r = state.db.reader();
        get_file_location(&r, file_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("file {file_id} not found"))?
    };
    if !loc.is_available {
        return Err("source root unreachable".into());
    }
    let djvused_bin = state
        .config
        .read()
        .expect("config poisoned")
        .djvused_bin
        .clone();

    tokio::task::spawn_blocking(move || {
        extract_djvu_single_page_spans(&loc.abs_path, page_ord, djvused_bin.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn open_file_external(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    page_ord: Option<u32>,
    template_override: Option<String>,
) -> Result<(), String> {
    let loc = {
        let r = state.db.reader();
        get_file_location(&r, file_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("file {file_id} not found"))?
    };
    if !loc.is_available || !loc.abs_path.exists() {
        return Err("Cartella sorgente non raggiungibile".into());
    }
    let cfg = state.config.read().expect("config poisoned");
    let configured_tpl = match loc.format.as_str() {
        "djvu" => cfg.external_djvu_command.as_deref(),
        _ => cfg.external_pdf_command.as_deref(),
    };
    let effective_tpl = template_override.as_deref().or(configured_tpl);
    open_external(&loc.abs_path, page_ord, effective_tpl).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reveal_file_in_folder(state: State<'_, Arc<AppState>>, file_id: i64) -> Result<(), String> {
    let loc = {
        let r = state.db.reader();
        get_file_location(&r, file_id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("file {file_id} not found"))?
    };
    if !loc.is_available || !loc.abs_path.exists() {
        return Err("Cartella sorgente non raggiungibile".into());
    }
    let mode = state.config.read().expect("config poisoned").linux_reveal_mode;
    reveal_in_folder(&loc.abs_path, mode).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_external_presets() -> Vec<ExternalReaderPreset> {
    external_reader_presets()
}

#[tauri::command]
pub fn get_ui_settings(state: State<'_, Arc<AppState>>) -> UiSettingsPayload {
    let cfg = state.config.read().expect("config poisoned");
    UiSettingsPayload {
        viewer_open_in_separate_window: cfg.viewer_open_in_separate_window,
        external_pdf_command: cfg.external_pdf_command.clone(),
        external_djvu_command: cfg.external_djvu_command.clone(),
        linux_reveal_mode: cfg.linux_reveal_mode,
        max_worker_threads: cfg.max_worker_threads,
        cache_limit_mb: cfg.cache_limit_mb,
        polite_email: load_polite_email(&cfg),
        network_lookups_enabled: cfg.network_lookups_enabled,
        mrlookup_enabled_single: cfg.mrlookup_enabled_single,
        mrlookup_enabled_bulk: cfg.mrlookup_enabled_bulk,
        zbmath_enabled: cfg.zbmath_enabled,
        crossref_enabled: cfg.crossref_enabled,
        arxiv_enabled: cfg.arxiv_enabled,
        openlibrary_enabled: cfg.openlibrary_enabled,
        dblp_enabled: cfg.dblp_enabled,
        openalex_enabled: cfg.openalex_enabled,
        datacite_enabled: cfg.datacite_enabled,
        inspirehep_enabled: cfg.inspirehep_enabled,
        europepmc_enabled: cfg.europepmc_enabled,
        semanticscholar_enabled: cfg.semanticscholar_enabled,
        metadata_source_priority: normalize_metadata_source_priority(&cfg.metadata_source_priority),
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
    }
}

#[tauri::command]
pub fn save_ui_settings(
    state: State<'_, Arc<AppState>>,
    settings: UiSettingsPayload,
) -> Result<(), String> {
    let mut cfg: AppConfig = state.config.read().expect("config poisoned").clone();
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
    cfg.openlibrary_enabled = settings.openlibrary_enabled;
    cfg.dblp_enabled = settings.dblp_enabled;
    cfg.openalex_enabled = settings.openalex_enabled;
    cfg.datacite_enabled = settings.datacite_enabled;
    cfg.inspirehep_enabled = settings.inspirehep_enabled;
    cfg.europepmc_enabled = settings.europepmc_enabled;
    cfg.semanticscholar_enabled = settings.semanticscholar_enabled;
    cfg.metadata_source_priority =
        normalize_metadata_source_priority(&settings.metadata_source_priority);
    cfg.ml_profile = ModelProfile::from_str_lossy(&settings.ml_profile).as_str().to_string();
    cfg.reranker_enabled = settings.reranker_enabled;
    cfg.model_idle_unload_secs = settings.model_idle_unload_secs.clamp(30, 3600);
    cfg.hybrid_top_n = settings.hybrid_top_n.clamp(10, 200);
    cfg.rerank_top_m_light = settings.rerank_top_m_light.clamp(5, 100);
    cfg.rerank_top_m_quality = settings.rerank_top_m_quality.clamp(4, 60);
    cfg.search_top_k_docs = settings.search_top_k_docs;
    cfg.search_min_score = settings.search_min_score.clamp(0.0, 1.0);
    cfg.doc_score_aggregation = match DocScoreAggregation::from_str_lossy(&settings.doc_score_aggregation) {
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
    cfg.save(&state.paths.config_path).map_err(|e| e.to_string())?;

    save_polite_email(settings.polite_email.as_deref(), Some(&state.paths.config_path))
        .map_err(|e| e.to_string())?;
    let reloaded = AppConfig::load_or_default(&state.paths.config_path).map_err(|e| e.to_string())?;
    let clients = Arc::new(WaterfallClients::from_config(&reloaded).map_err(|e| e.to_string())?);

    *state.clients.write().expect("clients poisoned") = clients;
    *state.config.write().expect("config poisoned") = reloaded;

    if prev_profile != cfg.ml_profile {
        state.embedder_slot.unload();
        state.reranker_slot.unload();
        spawn_background_embedder(Arc::clone(state.inner()));
    }
    AppState::refresh_watcher(&state, None);
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub fn open_detached_viewer_window(
    app: AppHandle,
    file_id: i64,
    page_ord: u32,
    title: String,
    bboxes_json: Option<String>,
) -> Result<(), String> {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let label = format!("viewer-{file_id}-{ms}");
    let mut url = format!("index.html?viewerFileId={file_id}&page={page_ord}");
    if let Some(b) = bboxes_json {
        // Safe query encoding via simple hex/base or standard URL encoding
        let encoded: String = b
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect();
        url.push_str(&format!("&bboxes={encoded}"));
    }
    WebviewWindowBuilder::new(&app, label, WebviewUrl::App(url.into()))
        .title(format!("{title} — Papero"))
        .inner_size(1024.0, 820.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct BulkMetadataProgressEvent {
    pub current: usize,
    pub total: usize,
    pub updated: usize,
    pub candidates: usize,
    pub unchanged: usize,
    pub failed: usize,
    pub current_title: String,
    pub finished: bool,
    pub cancelled: bool,
}

#[tauri::command]
pub async fn search_metadata(
    state: State<'_, Arc<AppState>>,
    filter: MetadataSearchFilter,
) -> Result<Vec<DocumentListItem>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        search_documents_metadata(&r, &filter).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_document_metadata(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
) -> Result<DocumentMetadataView, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        core_get_document_metadata(&db, document_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_document_metadata_verified_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    verified: bool,
) -> Result<DocumentMetadataView, String> {
    let db = Arc::clone(&state.db);
    let res = tokio::task::spawn_blocking(move || {
        set_document_metadata_verified(&db, document_id, verified).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(res)
}

#[tauri::command]
pub async fn save_document_metadata(
    state: State<'_, Arc<AppState>>,
    edit: ManualMetadataEdit,
    locked_fields: Option<Vec<String>>,
) -> Result<DocumentMetadataView, String> {
    let db = Arc::clone(&state.db);
    let view = tokio::task::spawn_blocking(move || {
        let view =
            apply_manual_metadata_edit_with_locks(&db, &edit, locked_fields.as_deref(), false)
                .map_err(|e| e.to_string())?;
        let _ = run_duplicate_detection(&db);
        Ok::<_, String>(view)
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(view)
}

#[tauri::command]
pub async fn save_and_fetch_document_metadata(
    state: State<'_, Arc<AppState>>,
    edit: ManualMetadataEdit,
    locked_fields: Option<Vec<String>>,
    reset_unlocked: Option<bool>,
) -> Result<SaveAndFetchResult, String> {
    let db_save = Arc::clone(&state.db);
    let edit_doc_id = edit.document_id;
    let reset = reset_unlocked.unwrap_or(true);
    let blocked_by_verified = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let verified = {
            let r = db_save.reader();
            is_document_metadata_verified(&r, edit_doc_id).map_err(|e| e.to_string())?
        };
        if verified && reset {
            // Protect confirmed metadata: persist explicit edits & locks without resetting verification or overwriting unlocked fields
            apply_manual_metadata_edit_with_locks(&db_save, &edit, locked_fields.as_deref(), false)
                .map_err(|e| e.to_string())?;
            return Ok(true);
        }
        apply_manual_metadata_edit_with_locks(&db_save, &edit, locked_fields.as_deref(), reset)
            .map_err(|e| e.to_string())?;
        Ok(false)
    })
    .await
    .map_err(|e| e.to_string())??;

    if blocked_by_verified {
        let db_after = Arc::clone(&state.db);
        let metadata = tokio::task::spawn_blocking(move || {
            core_get_document_metadata(&db_after, edit_doc_id).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??;
        state.schedule_auto_backup();
        return Ok(SaveAndFetchResult {
            outcome: ResolveOutcome {
                document_id: edit_doc_id,
                status: "verified_locked".to_string(),
                source: None,
                candidates_count: 0,
            },
            metadata,
        });
    }

    let cfg = state.config.read().expect("config poisoned").clone();
    let clients = Arc::clone(&state.clients.read().expect("clients poisoned"));
    let outcome = resolve_document_metadata(
        &state.db,
        &clients,
        &cfg,
        edit_doc_id,
        MetadataLookupScope::SingleRecordUserAction,
    )
    .await
    .map_err(|e| e.to_string())?;

    let db_after = Arc::clone(&state.db);
    let metadata = tokio::task::spawn_blocking(move || {
        let _ = run_duplicate_detection(&db_after);
        core_get_document_metadata(&db_after, edit_doc_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    state.schedule_auto_backup();
    Ok(SaveAndFetchResult { outcome, metadata })
}

#[tauri::command]
pub async fn fetch_document_metadata(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
) -> Result<ResolveOutcome, String> {
    let cfg = state.config.read().expect("config poisoned").clone();
    let clients = Arc::clone(&state.clients.read().expect("clients poisoned"));
    let outcome = resolve_document_metadata(
        &state.db,
        &clients,
        &cfg,
        document_id,
        MetadataLookupScope::SingleRecordUserAction,
    )
    .await
    .map_err(|e| e.to_string())?;
    if outcome.status == "updated" || outcome.status == "candidates_found" {
        let db_dedup = Arc::clone(&state.db);
        let _ = tokio::task::spawn_blocking(move || run_duplicate_detection(&db_dedup)).await;
        state.schedule_auto_backup();
    }
    Ok(outcome)
}

#[tauri::command]
pub async fn fetch_all_metadata_cmd(
    state: State<'_, Arc<AppState>>,
    only_unresolved: Option<bool>,
    on_event: Channel<BulkMetadataProgressEvent>,
) -> Result<BulkMetadataProgressEvent, String> {
    let cfg = state.config.read().expect("config poisoned").clone();
    if !cfg.network_lookups_enabled {
        return Err(
            "Ricerche metadati online disabilitate nelle Impostazioni -> Metadati & Rete.".to_string(),
        );
    }
    let clients = Arc::clone(&state.clients.read().expect("clients poisoned"));
    state.metadata_batch_cancel.store(false, Ordering::SeqCst);

    let db_list = Arc::clone(&state.db);
    let unresolved_flag = only_unresolved.unwrap_or(true);
    let docs = tokio::task::spawn_blocking(move || -> Result<Vec<(i64, String)>, String> {
        let r = db_list.reader();
        let mut items =
            list_documents_for_bulk_metadata(&r, unresolved_flag).map_err(|e| e.to_string())?;
        // If user clicked the button and 0 heuristic docs remain, re-scan all documents!
        if items.is_empty() && unresolved_flag {
            items = list_documents_for_bulk_metadata(&r, false).map_err(|e| e.to_string())?;
        }
        Ok(items)
    })
    .await
    .map_err(|e| e.to_string())??;

    let total = docs.len();
    let mut updated = 0usize;
    let mut candidates = 0usize;
    let mut unchanged = 0usize;
    let mut failed = 0usize;
    let mut cancelled = false;

    for (idx, (doc_id, title)) in docs.into_iter().enumerate() {
        if state.metadata_batch_cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let _ = on_event.send(BulkMetadataProgressEvent {
            current: idx + 1,
            total,
            updated,
            candidates,
            unchanged,
            failed,
            current_title: title.clone(),
            finished: false,
            cancelled: false,
        });

        match resolve_document_metadata(
            &state.db,
            &clients,
            &cfg,
            doc_id,
            MetadataLookupScope::BulkBackgroundScan,
        )
        .await
        {
            Ok(out) => {
                match out.status.as_str() {
                    "updated" => updated += 1,
                    "candidates_found" => candidates += 1,
                    _ => unchanged += 1,
                }
                let db_clean = Arc::clone(&state.db);
                let _ = tokio::task::spawn_blocking(move || {
                    let w = db_clean.writer();
                    let _ = clear_metadata_jobs_for_doc(&w, doc_id);
                })
                .await;
            }
            Err(_) => {
                failed += 1;
            }
        }
    }

    if updated > 0 {
        let db_dedup = Arc::clone(&state.db);
        let _ = tokio::task::spawn_blocking(move || run_duplicate_detection(&db_dedup)).await;
    }
    if updated > 0 || candidates > 0 {
        state.schedule_auto_backup();
    }

    let summary = BulkMetadataProgressEvent {
        current: if cancelled { updated + candidates + unchanged + failed } else { total },
        total,
        updated,
        candidates,
        unchanged,
        failed,
        current_title: String::new(),
        finished: true,
        cancelled,
    };
    let _ = on_event.send(summary.clone());
    Ok(summary)
}

#[tauri::command]
pub fn cancel_metadata_batch_cmd(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.metadata_batch_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub async fn confirm_candidate(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    candidate_id: i64,
) -> Result<DocumentMetadataView, String> {
    let db = Arc::clone(&state.db);
    let view = tokio::task::spawn_blocking(move || {
        let view =
            confirm_metadata_candidate(&db, document_id, candidate_id).map_err(|e| e.to_string())?;
        let _ = run_duplicate_detection(&db);
        Ok::<_, String>(view)
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(view)
}

#[tauri::command]
pub async fn dismiss_candidate(
    state: State<'_, Arc<AppState>>,
    candidate_id: i64,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        dismiss_metadata_candidate(&db, candidate_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn get_document_bibtex(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
) -> Result<String, String> {
    let view = core_get_document_metadata(&state.db, document_id).map_err(|e| e.to_string())?;
    let rec = metadata_view_to_record(&view);
    Ok(format_mrlookup_bibtex(&rec, &BibtexExportOptions::default()))
}

fn load_export_views(state: &AppState, document_ids: &[i64]) -> Result<Vec<DocumentMetadataView>, String> {
    let ids = if document_ids.is_empty() {
        let r = state.db.reader();
        list_all_document_ids(&r).map_err(|e| e.to_string())?
    } else {
        document_ids.to_vec()
    };

    let mut views = Vec::with_capacity(ids.len());
    for id in ids {
        if let Ok(v) = core_get_document_metadata(&state.db, id) {
            views.push(v);
        }
    }
    Ok(views)
}

#[tauri::command]
pub fn export_bibtex_many(
    state: State<'_, Arc<AppState>>,
    document_ids: Vec<i64>,
    file_path: Option<String>,
) -> Result<String, String> {
    let views = load_export_views(&state, &document_ids)?;
    let out = export_documents_bibtex(&views, &BibtexExportOptions::default());
    if let Some(p) = file_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        std::fs::write(p, &out).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[tauri::command]
pub fn export_csv_many(
    state: State<'_, Arc<AppState>>,
    document_ids: Vec<i64>,
    file_path: Option<String>,
) -> Result<String, String> {
    let views = load_export_views(&state, &document_ids)?;
    let out = export_documents_csv(&views);
    if let Some(p) = file_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        std::fs::write(p, &out).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[tauri::command]
pub fn import_metadata_file(
    state: State<'_, Arc<AppState>>,
    path: Option<String>,
    content: Option<String>,
    format: Option<String>,
    target_document_id: Option<i64>,
) -> Result<ImportMetadataReport, String> {
    let raw = if let Some(text) = content.filter(|s| !s.trim().is_empty()) {
        text
    } else if let Some(p) = path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        std::fs::read_to_string(p).map_err(|e| format!("read {p}: {e}"))?
    } else {
        return Err("Provide file path or content to import".into());
    };

    let detected_fmt = format
        .as_deref()
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| {
            if raw.trim_start().starts_with('[') || raw.trim_start().starts_with('{') {
                "csl_json".to_string()
            } else {
                "bib".to_string()
            }
        });

    let records = if detected_fmt == "csl_json" || detected_fmt == "json" {
        import_zotero_csl_json(&raw).map_err(|e| e.to_string())?
    } else {
        import_bibtex(&raw)
    };

    let total_entries = records.len();
    let matched_documents = import_metadata_records_into_library(
        &state.db,
        &records,
        target_document_id,
    )
    .map_err(|e| e.to_string())?;
    let _ = run_duplicate_detection(&state.db);
    if matched_documents > 0 {
        state.schedule_auto_backup();
    }
    Ok(ImportMetadataReport {
        total_entries,
        matched_documents,
        unmatched_entries: total_entries.saturating_sub(matched_documents),
    })
}

#[tauri::command]
pub fn list_duplicates(state: State<'_, Arc<AppState>>) -> Result<Vec<DuplicateGroupView>, String> {
    list_duplicate_groups(&state.db).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn run_dedup_scan(state: State<'_, Arc<AppState>>) -> Result<DuplicateReport, String> {
    run_duplicate_detection(&state.db).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn merge_duplicates(
    state: State<'_, Arc<AppState>>,
    group_id: i64,
    keep_document_id: i64,
) -> Result<(), String> {
    merge_duplicate_group(&state.db, group_id, keep_document_id).map_err(|e| e.to_string())?;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub fn resolve_duplicate(
    state: State<'_, Arc<AppState>>,
    group_id: i64,
    decision: String,
) -> Result<(), String> {
    resolve_duplicate_group(&state.db, group_id, &decision).map_err(|e| e.to_string())?;
    state.schedule_auto_backup();
    Ok(())
}

fn build_ml_status(state: &AppState) -> MlEnvironmentStatus {
    let cfg = state.config.read().expect("config poisoned");
    let ort_path = resolved_onnxruntime_path(cfg.onnxruntime_lib_path.as_deref());
    let active_profile = ModelProfile::from_str_lossy(&cfg.ml_profile);
    MlEnvironmentStatus {
        onnxruntime_found: ort_path.is_some(),
        onnxruntime_path: ort_path.map(|p| p.to_string_lossy().into_owned()),
        embedder_loaded: state.embedder_slot.is_loaded(),
        reranker_loaded: state.reranker_slot.is_loaded(),
        rss_mb: current_process_rss_bytes().unwrap_or(0) as f64 / (1024.0 * 1024.0),
        active_profile,
        profiles: core_list_model_statuses(&state.paths.models_dir),
    }
}

fn spawn_background_embedder(state: Arc<AppState>) {
    let cfg = state.config.read().expect("config poisoned").clone();
    let profile = ModelProfile::from_str_lossy(&cfg.ml_profile);
    if !profile_is_ready(&state.paths.models_dir, profile)
        || resolved_onnxruntime_path(cfg.onnxruntime_lib_path.as_deref()).is_none()
    {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        let vindex = state.vector_index_for(profile);
        let _ = run_embed_queue(
            &state.db,
            &vindex,
            &state.embedder_slot,
            &state.paths.models_dir,
            profile,
            &cfg,
            None,
        );
    });
}

#[tauri::command]
pub async fn search_content_hybrid(
    state: State<'_, Arc<AppState>>,
    query: String,
    root_ids: Option<Vec<String>>,
    rerank_override: Option<bool>,
    limit_override: Option<usize>,
    work_category: Option<String>,
) -> Result<HybridSearchResponse, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let cfg = st.config.read().expect("config poisoned").clone();
        let profile = ModelProfile::from_str_lossy(&cfg.ml_profile);
        let rerank_top_m = match profile {
            ModelProfile::Light => cfg.rerank_top_m_light,
            ModelProfile::Quality => cfg.rerank_top_m_quality,
        };
        let opts = HybridSearchOptions {
            top_n_per_branch: cfg.hybrid_top_n,
            rerank_enabled: rerank_override.unwrap_or(cfg.reranker_enabled),
            rerank_top_m,
            top_k_docs: limit_override.unwrap_or(cfg.search_top_k_docs),
            min_score: cfg.search_min_score,
            rrf_k: 60.0,
            aggregation: DocScoreAggregation::from_str_lossy(&cfg.doc_score_aggregation),
            work_category,
        };

        let ort_ready = resolved_onnxruntime_path(cfg.onnxruntime_lib_path.as_deref()).is_some();
        let models_ready = profile_is_ready(&st.paths.models_dir, profile);
        let r = st.db.reader();

        if ort_ready && models_ready {
            let vindex = st.vector_index_for(profile);
            let models_dir = st.paths.models_dir.clone();
            let ort_lib = cfg.onnxruntime_lib_path.clone();
            let threads = cfg.max_worker_threads.max(1);
            let ctx = HybridSearchContext {
                vector_index: Some(vindex.as_ref()),
                embedder_slot: Some(st.embedder_slot.as_ref()),
                embedder_loader: Some(|| {
                    OnnxEmbedder::load_isolated(&models_dir, profile, ort_lib.as_deref(), threads)
                }),
                reranker_slot: if opts.rerank_enabled {
                    Some(st.reranker_slot.as_ref())
                } else {
                    None
                },
                reranker_loader: Some(|| {
                    OnnxReranker::load_isolated(&models_dir, profile, ort_lib.as_deref(), threads)
                }),
            };
            hybrid_search(&r, &st.bm25, &query, root_ids.as_deref(), &opts, Some(ctx))
                .map_err(|e| e.to_string())
        } else {
            type LoaderFn<T> = fn() -> papero_core::error::Result<T>;
            let none_ctx: Option<
                HybridSearchContext<'_, LoaderFn<OnnxEmbedder>, LoaderFn<OnnxReranker>>,
            > = None;
            hybrid_search(&r, &st.bm25, &query, root_ids.as_deref(), &opts, none_ctx)
                .map_err(|e| e.to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn list_model_statuses(state: State<'_, Arc<AppState>>) -> MlEnvironmentStatus {
    build_ml_status(&state)
}

#[tauri::command]
pub async fn download_ml_profile(
    state: State<'_, Arc<AppState>>,
    profile: String,
    on_event: Channel<DownloadProgressEvent>,
) -> Result<MlEnvironmentStatus, String> {
    let parsed_profile = ModelProfile::from_str_lossy(&profile);
    let models_dir = state.paths.models_dir.clone();
    download_model_profile(&models_dir, parsed_profile, |ev| {
        let _ = on_event.send(ev);
    })
    .await
    .map_err(|e| e.to_string())?;

    spawn_background_embedder(Arc::clone(&state));
    Ok(build_ml_status(&state))
}

#[tauri::command]
pub fn unload_ml_models(state: State<'_, Arc<AppState>>) -> MlEnvironmentStatus {
    state.embedder_slot.unload();
    state.reranker_slot.unload();
    build_ml_status(&state)
}

#[tauri::command]
pub async fn import_alt_formula_text(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    markdown_text: String,
) -> Result<AltFormulaImportReport, String> {
    let st = Arc::clone(&state);
    let report = tokio::task::spawn_blocking(move || {
        let cfg = st.config.read().expect("config poisoned").clone();
        let w = st.db.writer();
        import_alt_formula_markdown(
            &w,
            &st.bm25,
            &st.paths.cache_dir,
            file_id,
            &markdown_text,
            cfg.chunk_target_tokens,
            cfg.chunk_overlap_tokens,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    spawn_background_embedder(Arc::clone(&state));
    state.schedule_auto_backup();
    Ok(report)
}

#[tauri::command]
pub fn get_msc_tree(state: State<'_, Arc<AppState>>) -> Result<Vec<MscTreeNode>, String> {
    let r = state.db.reader();
    build_msc_tree(&r).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn run_knn_msc_proposals(
    state: State<'_, Arc<AppState>>,
    k: Option<usize>,
) -> Result<KnnProposeReport, String> {
    let st = Arc::clone(&state);
    let rep = tokio::task::spawn_blocking(move || {
        let raw_prof = st.config.read().expect("config poisoned").ml_profile.clone();
        let profile = ModelProfile::from_str_lossy(&raw_prof);
        let vindex = st.vector_index_for(profile);
        let w = st.db.writer();
        propose_msc_knn(&w, &vindex, k.unwrap_or(7), 0.20).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if rep.proposed_count > 0 {
        state.schedule_auto_backup();
    }
    Ok(rep)
}

#[tauri::command]
pub fn confirm_msc_classification(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    msc_code: Option<String>,
) -> Result<(), String> {
    {
        let w = state.db.writer();
        confirm_document_msc(&w, document_id, msc_code.as_deref()).map_err(|e| e.to_string())?;
    }
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub fn reject_msc_proposal(state: State<'_, Arc<AppState>>, document_id: i64) -> Result<(), String> {
    {
        let w = state.db.writer();
        reject_proposed_msc(&w, document_id).map_err(|e| e.to_string())?;
    }
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn run_topic_clustering_cmd(
    state: State<'_, Arc<AppState>>,
    requested_k: Option<usize>,
) -> Result<ClusteringReport, String> {
    let st = Arc::clone(&state);
    let rep = tokio::task::spawn_blocking(move || {
        let raw_prof = st.config.read().expect("config poisoned").ml_profile.clone();
        let profile = ModelProfile::from_str_lossy(&raw_prof);
        let vindex = st.vector_index_for(profile);
        let w = st.db.writer();
        run_topic_clustering(&w, &vindex, profile, requested_k).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(rep)
}

#[tauri::command]
pub fn list_topic_clusters(state: State<'_, Arc<AppState>>) -> Result<Vec<TopicClusterView>, String> {
    let r = state.db.reader();
    core_list_topic_clusters(&r).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_topic_cluster_cmd(
    state: State<'_, Arc<AppState>>,
    cluster_id: i64,
    label: String,
) -> Result<(), String> {
    {
        let w = state.db.writer();
        rename_topic_cluster(&w, cluster_id, &label).map_err(|e| e.to_string())?;
    }
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub fn set_document_cluster_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    cluster_id: i64,
) -> Result<(), String> {
    {
        let w = state.db.writer();
        set_document_cluster(&w, document_id, cluster_id).map_err(|e| e.to_string())?;
    }
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub fn remove_document_cluster_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    cluster_id: i64,
) -> Result<(), String> {
    {
        let w = state.db.writer();
        remove_document_from_cluster(&w, document_id, cluster_id).map_err(|e| e.to_string())?;
    }
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub fn list_all_tags_cmd(state: State<'_, Arc<AppState>>) -> Result<Vec<TagSummary>, String> {
    let r = state.db.reader();
    list_all_tags(&r).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_document_tag_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    tag_name: String,
) -> Result<(), String> {
    {
        let w = state.db.writer();
        add_tag_to_document(&w, document_id, &tag_name).map_err(|e| e.to_string())?;
    }
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub fn remove_document_tag_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    tag_name: String,
) -> Result<(), String> {
    {
        let w = state.db.writer();
        remove_tag_from_document(&w, document_id, &tag_name).map_err(|e| e.to_string())?;
    }
    state.schedule_auto_backup();
    Ok(())
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
            .ok_or_else(|| "Nessun percorso di backup configurato. Crea prima un backup.".to_string())?
    };
    AppState::run_backup_and_record(&state, &dest).await
}

#[tauri::command]
pub async fn restore_backup_cmd(
    state: State<'_, Arc<AppState>>,
    src_path: String,
) -> Result<RestoreReport, String> {
    let _guard = state.backup_mutex.lock().await;
    state.auto_backup_dirty.store(false, Ordering::SeqCst);
    let st = Arc::clone(&state);
    st.embedder_slot.unload();
    st.reranker_slot.unload();
    let rep = tokio::task::spawn_blocking(move || {
        restore_backup_bundle(&st.paths, Path::new(&src_path)).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if let Ok(reloaded) = AppConfig::load_or_default(&state.paths.config_path) {
        *state.config.write().expect("config poisoned") = reloaded;
    }
    Ok(rep)
}

#[tauri::command]
pub fn get_ocr_status(state: State<'_, Arc<AppState>>) -> OcrEnvironmentStatus {
    let cfg = state.config.read().expect("config poisoned");
    detect_tesseract(cfg.tesseract_bin.as_deref(), cfg.ocr_languages.as_deref())
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

#[derive(Debug, Clone, Serialize)]
pub struct LlmKeyStatus {
    pub anthropic_present: bool,
    pub openai_present: bool,
    pub gemini_present: bool,
}

fn load_provider_key_from_keyring(provider: LlmProvider) -> Result<Option<SecretApiKey>, String> {
    match provider.keyring_account_key() {
        Some(acct) => load_secret_key(acct).map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

fn resolve_effective_llm_key(
    provider: LlmProvider,
    api_key_override: Option<String>,
) -> Result<Option<SecretApiKey>, String> {
    if !provider.is_cloud() {
        return Ok(None);
    }
    if let Some(raw) = api_key_override {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(Some(SecretApiKey::new(trimmed.to_string())));
        }
    }
    load_provider_key_from_keyring(provider)
}

#[tauri::command]
pub fn set_root_cloud_exclusion(
    state: State<'_, Arc<AppState>>,
    root_id: String,
    excluded: bool,
) -> Result<(), String> {
    {
        let w = state.db.writer();
        set_root_exclude_from_cloud_llm(&w, &root_id, excluded).map_err(|e| e.to_string())?;
    }
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub fn get_llm_key_status() -> Result<LlmKeyStatus, String> {
    Ok(LlmKeyStatus {
        anthropic_present: load_provider_key_from_keyring(LlmProvider::Anthropic)
            .ok()
            .flatten()
            .is_some(),
        openai_present: load_provider_key_from_keyring(LlmProvider::OpenAi)
            .ok()
            .flatten()
            .is_some(),
        gemini_present: load_provider_key_from_keyring(LlmProvider::Gemini)
            .ok()
            .flatten()
            .is_some(),
    })
}

#[tauri::command]
pub fn save_llm_api_key(provider: LlmProvider, api_key: String) -> Result<(), String> {
    let acct = provider
        .keyring_account_key()
        .ok_or_else(|| "Selected provider does not use a cloud API key.".to_string())?;
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err("API key cannot be empty.".to_string());
    }
    let secret = SecretApiKey::new(trimmed.to_string());
    save_secret_key(acct, &secret).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_llm_api_key(provider: LlmProvider) -> Result<(), String> {
    let acct = provider
        .keyring_account_key()
        .ok_or_else(|| "Selected provider does not use a cloud API key.".to_string())?;
    delete_secret_key(acct).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn verify_llm_key_cmd(
    state: State<'_, Arc<AppState>>,
    provider: LlmProvider,
    api_key_override: Option<String>,
    ollama_url_override: Option<String>,
) -> Result<LlmKeyVerifyResult, String> {
    let ollama_url = ollama_url_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| state.config.read().expect("config poisoned").llm_ollama_url.clone());
    let key = resolve_effective_llm_key(provider, api_key_override)?;
    Ok(verify_provider_key(provider, key.as_ref(), Some(&ollama_url), None).await)
}

#[tauri::command]
pub async fn list_llm_models_cmd(
    state: State<'_, Arc<AppState>>,
    provider: LlmProvider,
    ollama_url_override: Option<String>,
    api_key_override: Option<String>,
) -> Result<Vec<LlmModelInfo>, String> {
    let ollama_url = ollama_url_override
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| state.config.read().expect("config poisoned").llm_ollama_url.clone());
    let key = resolve_effective_llm_key(provider, api_key_override)?;
    list_models(provider, key.as_ref(), Some(&ollama_url), None)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn preview_llm_prompt_cmd(
    state: State<'_, Arc<AppState>>,
    query: String,
    results: Vec<ContentSearchDocResult>,
    max_passages: Option<usize>,
    max_input_tokens: Option<usize>,
) -> Result<LlmPromptPreview, String> {
    let cfg = state.config.read().expect("config poisoned").clone();
    let reader = state.db.reader();
    let prepared = prepare_grounded_prompt(
        Some(&reader),
        &cfg,
        &query,
        &results,
        max_passages,
        max_input_tokens,
    )
    .map_err(|e| e.to_string())?;
    Ok(prepared.preview)
}

#[tauri::command]
pub fn accept_llm_cloud_consent_cmd(
    state: State<'_, Arc<AppState>>,
    provider: LlmProvider,
) -> Result<(), String> {
    let mut cfg = state.config.write().expect("config poisoned");
    accept_cloud_consent(&mut cfg, provider);
    cfg.save(&state.paths.config_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn generate_llm_answer_cmd(
    state: State<'_, Arc<AppState>>,
    query: String,
    results: Vec<ContentSearchDocResult>,
    max_passages: Option<usize>,
    max_input_tokens: Option<usize>,
) -> Result<LlmAnswerResult, String> {
    let st = Arc::clone(&state);
    let cfg = st.config.read().expect("config poisoned").clone();
    let key = if cfg.llm_provider.is_cloud() {
        load_provider_key_from_keyring(cfg.llm_provider)?
    } else {
        None
    };
    generate_grounded_answer(
        Some(&st.db),
        &cfg,
        key.as_ref(),
        &query,
        &results,
        max_passages,
        max_input_tokens,
        None,
    )
    .await
    .map_err(|e| e.to_string())
}
