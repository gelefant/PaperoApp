use crate::commands::search::spawn_background_embedder;
use crate::state::AppState;
use papero_core::db::repos::roots::{
    insert_source_root, list_source_roots, normalize_root_dir_path, rebind_source_root_path,
    record_local_root_path, remove_local_root_path, remove_source_root_with_cleanup,
    set_root_enabled, set_root_exclude_from_cloud_llm, NewSourceRoot, SourceRoot,
};
use papero_core::db::repos::scans::ScanSummary;
use papero_core::dedup::run_duplicate_detection;
use papero_core::fs::roots::new_root_id;
use papero_core::fs::scanner::{rescan_sources, ScanControl};
use papero_core::index::worker::{run_extract_bm25_queue, WorkerBatchReport};
use papero_core::metadata::resolver::run_pending_metadata_jobs_cancellable;
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{Emitter, State};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum ScanProgressEvent {
    Started,
    ScanDone { summary: ScanSummary },
    IndexingDone { report: WorkerBatchReport },
    Finished,
    Error { message: String },
}

#[tauri::command]
pub async fn list_roots(state: State<'_, Arc<AppState>>) -> Result<Vec<SourceRoot>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        list_source_roots(&r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn validate_and_canonicalize_dir(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let p = Path::new(trimmed);
    if !p.is_dir() {
        return Err(format!("Path is not a readable directory: {trimmed}"));
    }
    Ok(normalize_root_dir_path(p))
}

#[tauri::command]
pub async fn add_source_root(
    state: State<'_, Arc<AppState>>,
    path: String,
    label: Option<String>,
    is_network_or_cloud: Option<bool>,
) -> Result<SourceRoot, String> {
    let st = Arc::clone(&state);
    let root = tokio::task::spawn_blocking(move || -> Result<SourceRoot, String> {
        let canonical = validate_and_canonicalize_dir(&path)?;
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
            let w = st.db.writer();
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
        let _ = record_local_root_path(&st.paths.local_roots_path(), &root.id, &root.root_path);
        Ok(root)
    })
    .await
    .map_err(|e| e.to_string())??;
    AppState::refresh_watcher(&state, None);
    state.schedule_auto_backup();
    Ok(root)
}

#[tauri::command]
pub async fn rebind_source_root(
    state: State<'_, Arc<AppState>>,
    root_id: String,
    new_path: String,
) -> Result<SourceRoot, String> {
    let st = Arc::clone(&state);
    let root = tokio::task::spawn_blocking(move || -> Result<SourceRoot, String> {
        let canonical = validate_and_canonicalize_dir(&new_path)?;
        let w = st.db.writer();
        rebind_source_root_path(&w, &st.paths.local_roots_path(), &root_id, &canonical)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.invalidate_viewer_file_cache();
    AppState::refresh_watcher(&state, None);
    state.schedule_auto_backup();
    Ok(root)
}

#[tauri::command]
pub async fn toggle_source_root(
    state: State<'_, Arc<AppState>>,
    root_id: String,
    enabled: bool,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        set_root_enabled(&w, &root_id, enabled).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.invalidate_viewer_file_cache();
    AppState::refresh_watcher(&state, None);
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn delete_source_root(
    state: State<'_, Arc<AppState>>,
    root_id: String,
) -> Result<(), String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let report = {
            let w = st.db.writer();
            remove_source_root_with_cleanup(&w, &root_id).map_err(|e| e.to_string())?
        };
        if !report.deleted_passage_ids.is_empty() || !report.deleted_document_ids.is_empty() {
            let _ = st.bm25.delete_passages(&report.deleted_passage_ids);
            for doc_id in &report.deleted_document_ids {
                let _ = st.bm25.delete_document(*doc_id as u64);
            }
            let _ = st.bm25.commit();
            if !report.deleted_passage_ids.is_empty() {
                let _ = st.vector_light.remove_keys(&report.deleted_passage_ids);
                let _ = st.vector_quality.remove_keys(&report.deleted_passage_ids);
            }
        }
        let _ = remove_local_root_path(&st.paths.local_roots_path(), &root_id);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.invalidate_viewer_file_cache();
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
    let _embed_pause = AppState::pause_embed_for_batch(state.inner());
    let ctrl = Arc::new(ScanControl::new());
    {
        let mut slot = state.scan_control.lock().expect("scan_control poisoned");
        *slot = Some(Arc::clone(&ctrl));
    }

    let db = Arc::clone(&state.db);
    let bm25 = Arc::clone(&state.bm25);
    let cfg = state.config.read().expect("config poisoned").clone();

    let _ = on_event.send(ScanProgressEvent::Started);

    let on_event_err = on_event.clone();
    let res = match tokio::task::spawn_blocking(move || -> Result<(), String> {
        let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
            let summary =
                match rescan_sources(&db, Some(&bm25), only_root_id.as_deref(), Some(&ctrl)) {
                    Ok(s) => s,
                    Err(e) => {
                        let msg = e.to_string();
                        let _ = on_event.send(ScanProgressEvent::Error {
                            message: msg.clone(),
                        });
                        return Err(msg);
                    }
                };
            let _ = on_event.send(ScanProgressEvent::ScanDone { summary });

            let report = match run_extract_bm25_queue(&db, &bm25, &cfg, Some(&ctrl)) {
                Ok(r) => r,
                Err(e) => {
                    let msg = e.to_string();
                    let _ = on_event.send(ScanProgressEvent::Error {
                        message: msg.clone(),
                    });
                    return Err(msg);
                }
            };
            let _ = on_event.send(ScanProgressEvent::IndexingDone { report });
            let _ = on_event.send(ScanProgressEvent::Finished);
            Ok(())
        }));
        match inner {
            Ok(r) => r,
            Err(payload) => {
                let detail = payload
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("unknown rescan panic");
                let msg = format!("Errore interno durante la scansione: {detail}");
                let _ = on_event_err.send(ScanProgressEvent::Error {
                    message: msg.clone(),
                });
                Err(msg)
            }
        }
    })
    .await
    {
        Ok(r) => r,
        Err(join_err) => Err(join_err.to_string()),
    };

    {
        let mut slot = state.scan_control.lock().expect("scan_control poisoned");
        *slot = None;
    }
    state.invalidate_viewer_file_cache();

    let st_bg = Arc::clone(&state);
    let cfg_bg = state.config.read().expect("config poisoned").clone();
    let clients_bg = Arc::clone(&state.clients.read().expect("clients poisoned"));
    if res.is_ok() && cfg_bg.network_lookups_enabled {
        state.bg_metadata_cancel.store(false, Ordering::SeqCst);
        let bg_embed_pause = AppState::pause_embed_for_batch(state.inner());
        tauri::async_runtime::spawn(async move {
            let _pause_keep = bg_embed_pause;
            let _meta_guard = st_bg.metadata_run_mutex.lock().await;
            if st_bg.bg_metadata_cancel.load(Ordering::SeqCst) {
                drop(_pause_keep);
                spawn_background_embedder(Arc::clone(&st_bg));
                return;
            }
            let _active = AppState::mark_metadata_batch_active(&st_bg);
            let run_res = tokio::select! {
                biased;
                _ = st_bg.metadata_cancel_notify.notified(), if st_bg.bg_metadata_cancel.load(Ordering::SeqCst) => {
                    Ok(0)
                }
                r = run_pending_metadata_jobs_cancellable(
                    &st_bg.db,
                    &clients_bg,
                    &cfg_bg,
                    2000,
                    Some(&st_bg.bg_metadata_cancel),
                ) => r,
            };
            drop(_active);
            drop(_meta_guard);
            drop(_pause_keep);

            if let Ok(cnt) = run_res {
                if cnt > 0 && !st_bg.bg_metadata_cancel.load(Ordering::SeqCst) {
                    let db_dedup = Arc::clone(&st_bg.db);
                    let _ =
                        tokio::task::spawn_blocking(move || run_duplicate_detection(&db_dedup))
                            .await;
                    st_bg.schedule_auto_backup();
                    if let Ok(guard) = st_bg.app_handle.lock() {
                        if let Some(app) = guard.as_ref() {
                            let _ = app.emit("library-fs-updated", ());
                        }
                    }
                }
            }
            if !st_bg.bg_metadata_cancel.load(Ordering::SeqCst) {
                spawn_background_embedder(Arc::clone(&st_bg));
            }
        });
        state.schedule_auto_backup();
    } else if res.is_ok() {
        drop(_embed_pause);
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
pub async fn set_root_cloud_exclusion(
    state: State<'_, Arc<AppState>>,
    root_id: String,
    excluded: bool,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        set_root_exclude_from_cloud_llm(&w, &root_id, excluded).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}
