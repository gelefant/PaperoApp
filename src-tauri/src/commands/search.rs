use crate::state::AppState;
use papero_core::db::repos::docs::{list_documents_for_ui, ContentSearchDocResult, DocumentListItem};
use papero_core::extract::alt_text::{import_alt_formula_markdown, AltFormulaImportReport};
use papero_core::index::hybrid::{
    hybrid_search, search_bm25_filtered, ContentSearchFilter, DocScoreAggregation,
    HybridSearchContext, HybridSearchOptions, HybridSearchResponse,
};
use papero_core::index::worker::{count_embed_progress, run_embed_queue_with_progress};
use papero_core::ml::{
    current_process_rss_bytes, download_model_profile,
    list_model_statuses as core_list_model_statuses, profile_is_ready, resolved_onnxruntime_path,
    DownloadProgressEvent, ModelProfile, ModelProfileStatus, OnnxEmbedder, OnnxReranker,
};
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{Emitter, State};

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
pub struct EmbedProgressStatus {
    pub profile: String,
    pub profile_ready: bool,
    pub total_files: i64,
    pub embedded_files: i64,
    pub pending_files: i64,
    pub running: bool,
    pub paused: bool,
    pub user_paused: bool,
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
pub async fn search_content_bm25(
    state: State<'_, Arc<AppState>>,
    query: String,
    root_ids: Option<Vec<String>>,
    limit: Option<usize>,
    work_category: Option<String>,
    filter: Option<ContentSearchFilter>,
) -> Result<Vec<ContentSearchDocResult>, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let base_limit = limit.unwrap_or(50);
        let mut eff_filter = filter.unwrap_or_default();
        if eff_filter.root_ids.is_none() && root_ids.is_some() {
            eff_filter.root_ids = root_ids;
        }
        if eff_filter.work_category.is_none() && work_category.is_some() {
            eff_filter.work_category = work_category;
        }
        let r = st.db.reader();
        search_bm25_filtered(&r, &st.bm25, &query, &eff_filter, base_limit)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
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

fn compute_embed_progress(state: &AppState) -> EmbedProgressStatus {
    let cfg = state.config.read().expect("config poisoned").clone();
    let profile = ModelProfile::from_str_lossy(&cfg.ml_profile);
    let profile_ready = profile_is_ready(&state.paths.models_dir, profile)
        && resolved_onnxruntime_path(cfg.onnxruntime_lib_path.as_deref()).is_some();
    let (total_files, embedded_files) =
        count_embed_progress(&state.db, profile).unwrap_or((0, 0));
    let pending_files = (total_files - embedded_files).max(0);
    let user_paused = state.embed_user_paused.load(Ordering::SeqCst);
    let paused = state.is_embed_effectively_paused();
    let running = state.embed_worker_running.load(Ordering::SeqCst);
    EmbedProgressStatus {
        profile: profile.as_str().to_string(),
        profile_ready,
        total_files,
        embedded_files,
        pending_files,
        running,
        paused,
        user_paused,
    }
}

fn emit_embed_progress(state: &AppState) {
    let status = compute_embed_progress(state);
    if let Ok(guard) = state.app_handle.lock() {
        if let Some(app) = guard.as_ref() {
            let _ = app.emit("papero://embed-progress", &status);
        }
    }
}

#[tauri::command]
pub async fn get_embed_progress_cmd(
    state: State<'_, Arc<AppState>>,
) -> Result<EmbedProgressStatus, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || compute_embed_progress(&st))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_embed_paused_cmd(
    state: State<'_, Arc<AppState>>,
    paused: bool,
) -> Result<EmbedProgressStatus, String> {
    state.set_embed_user_paused(paused);
    if !paused {
        spawn_background_embedder(Arc::clone(&state));
    }
    let st = Arc::clone(&state);
    let status = tokio::task::spawn_blocking(move || {
        let s = compute_embed_progress(&st);
        if let Ok(guard) = st.app_handle.lock() {
            if let Some(app) = guard.as_ref() {
                let _ = app.emit("papero://embed-progress", &s);
            }
        }
        s
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(status)
}

pub(crate) fn spawn_background_embedder(state: Arc<AppState>) {
    state.embed_worker_reschedule.store(true, Ordering::SeqCst);
    if state
        .embed_worker_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        struct RunningGuard(Arc<AppState>);
        impl Drop for RunningGuard {
            fn drop(&mut self) {
                self.0.embed_worker_running.store(false, Ordering::SeqCst);
                emit_embed_progress(&self.0);
            }
        }
        let _guard = RunningGuard(Arc::clone(&state));
        emit_embed_progress(&state);

        while state.embed_worker_reschedule.swap(false, Ordering::SeqCst) {
            let cfg = state.config.read().expect("config poisoned").clone();
            let profile = ModelProfile::from_str_lossy(&cfg.ml_profile);
            if !profile_is_ready(&state.paths.models_dir, profile)
                || resolved_onnxruntime_path(cfg.onnxruntime_lib_path.as_deref()).is_none()
            {
                break;
            }
            let ctrl = state.reset_embed_control();
            let vindex = state.vector_index_for(profile);
            let st_cb = Arc::clone(&state);
            let on_progress = move || {
                emit_embed_progress(&st_cb);
            };
            let embed_outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_embed_queue_with_progress(
                    &state.db,
                    &vindex,
                    &state.embedder_slot,
                    &state.paths.models_dir,
                    profile,
                    &cfg,
                    Some(&ctrl),
                    Some(&on_progress),
                )
            }));
            if let Ok(Ok(rep)) = embed_outcome {
                if rep.succeeded > 0 {
                    state.schedule_auto_backup();
                }
            }
            if ctrl.is_cancelled() && !state.embed_worker_reschedule.load(Ordering::SeqCst) {
                break;
            }
        }
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
    filter: Option<ContentSearchFilter>,
) -> Result<HybridSearchResponse, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let cfg = st.config.read().expect("config poisoned").clone();
        let profile = ModelProfile::from_str_lossy(&cfg.ml_profile);
        let rerank_top_m = match profile {
            ModelProfile::Light => cfg.rerank_top_m_light,
            ModelProfile::Quality => cfg.rerank_top_m_quality,
        };
        let mut eff_filter = filter.unwrap_or_default();
        if eff_filter.root_ids.is_none() && root_ids.is_some() {
            eff_filter.root_ids = root_ids.clone();
        }
        if eff_filter.work_category.is_none() && work_category.is_some() {
            eff_filter.work_category = work_category.clone();
        }
        let eff_roots = eff_filter.root_ids.clone();
        let opts = HybridSearchOptions {
            top_n_per_branch: cfg.hybrid_top_n,
            rerank_enabled: rerank_override.unwrap_or(cfg.reranker_enabled),
            rerank_top_m,
            top_k_docs: limit_override.unwrap_or(cfg.search_top_k_docs),
            min_score: cfg.search_min_score,
            rrf_k: 60.0,
            aggregation: DocScoreAggregation::from_str_lossy(&cfg.doc_score_aggregation),
            work_category,
            filter: eff_filter,
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
            hybrid_search(&r, &st.bm25, &query, eff_roots.as_deref(), &opts, Some(ctx))
                .map_err(|e| e.to_string())
        } else {
            type LoaderFn<T> = fn() -> papero_core::error::Result<T>;
            let none_ctx: Option<
                HybridSearchContext<'_, LoaderFn<OnnxEmbedder>, LoaderFn<OnnxReranker>>,
            > = None;
            hybrid_search(&r, &st.bm25, &query, eff_roots.as_deref(), &opts, none_ctx)
                .map_err(|e| e.to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_model_statuses(
    state: State<'_, Arc<AppState>>,
) -> Result<MlEnvironmentStatus, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || build_ml_status(&st))
        .await
        .map_err(|e| e.to_string())
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
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || build_ml_status(&st))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn unload_ml_models(
    state: State<'_, Arc<AppState>>,
) -> Result<MlEnvironmentStatus, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        st.cancel_embed_worker();
        st.embedder_slot.unload();
        st.reranker_slot.unload();
        build_ml_status(&st)
    })
    .await
    .map_err(|e| e.to_string())
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
