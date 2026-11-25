use crate::commands::search::spawn_background_embedder;
use crate::state::AppState;
use papero_core::db::repos::docs::DocumentListItem;
use papero_core::dedup::{
    list_duplicate_groups, merge_duplicate_group, resolve_duplicate_group, run_duplicate_detection,
    DuplicateGroupView, DuplicateReport,
};
use papero_core::metadata::io::{import_bibtex, import_zotero_csl_json};
use papero_core::metadata::models::DocumentMetadataView;
use papero_core::metadata::relations::{
    find_missing_cited_classics, get_document_relations, refresh_local_citations_for_doc,
    resolve_citation_snippet_in_library, CitationLinkItem, DocumentRelationsView,
    MissingClassicItem,
};
use papero_core::metadata::resolver::{
    apply_manual_metadata_edit_with_locks, clear_metadata_jobs_for_doc,
    confirm_metadata_candidate_with_options, dismiss_metadata_candidate,
    get_document_metadata as core_get_document_metadata, import_metadata_records_into_library,
    is_document_metadata_verified, list_documents_for_bulk_metadata, prepare_guided_search_edit,
    resolve_document_metadata, resolve_document_metadata_with_hints, search_documents_metadata,
    set_document_metadata_verified, GuidedSearchHints, ManualMetadataEdit, MetadataLookupScope,
    MetadataSearchFilter, ResolveOutcome, SaveAndFetchResult,
};
use papero_core::metadata::smart_paste::{parse_smart_paste, SmartPasteResult};
use papero_core::ml::ModelProfile;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::{Emitter, State};

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

#[derive(Debug, Clone, Serialize)]
pub struct ImportMetadataReport {
    pub total_entries: usize,
    pub matched_documents: usize,
    pub unmatched_entries: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SmartPasteApplyResult {
    pub kind: String,
    pub summary: String,
    pub outcome: Option<ResolveOutcome>,
    pub metadata: DocumentMetadataView,
}

pub(crate) fn spawn_background_dedup(state: &Arc<AppState>) {
    let st = Arc::clone(state);
    tauri::async_runtime::spawn(async move {
        let db = Arc::clone(&st.db);
        let rep = tokio::task::spawn_blocking(move || run_duplicate_detection(&db)).await;
        if let Ok(Ok(r)) = rep {
            if r.new_groups_created > 0 {
                if let Ok(guard) = st.app_handle.lock() {
                    if let Some(app) = guard.as_ref() {
                        let _ = app.emit("papero://duplicates-updated", ());
                    }
                }
            }
        }
    });
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
pub fn cancel_single_metadata_cmd(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    state.single_metadata_cancel_notify.notify_waiters();
    Ok(())
}

#[tauri::command]
pub async fn set_document_metadata_verified_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    verified: bool,
) -> Result<DocumentMetadataView, String> {
    if verified {
        state.single_metadata_cancel_notify.notify_waiters();
    }
    let db = Arc::clone(&state.db);
    let res = tokio::task::spawn_blocking(move || {
        set_document_metadata_verified(&db, document_id, verified).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    state.schedule_bibtex_auto_sync();
    Ok(res)
}

#[tauri::command]
pub async fn save_document_metadata(
    state: State<'_, Arc<AppState>>,
    edit: ManualMetadataEdit,
    locked_fields: Option<Vec<String>>,
) -> Result<DocumentMetadataView, String> {
    state.single_metadata_cancel_notify.notify_waiters();
    let db = Arc::clone(&state.db);
    let view = tokio::task::spawn_blocking(move || {
        apply_manual_metadata_edit_with_locks(&db, &edit, locked_fields.as_deref(), false)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    spawn_background_dedup(&state);
    state.schedule_auto_backup();
    state.schedule_bibtex_auto_sync();
    Ok(view)
}

#[tauri::command]
pub async fn save_and_fetch_document_metadata(
    state: State<'_, Arc<AppState>>,
    edit: ManualMetadataEdit,
    locked_fields: Option<Vec<String>>,
    reset_unlocked: Option<bool>,
    overwrite_query_fields: Option<bool>,
) -> Result<SaveAndFetchResult, String> {
    let cancel_wait = state.single_metadata_cancel_notify.notified();
    tokio::pin!(cancel_wait);
    cancel_wait.as_mut().enable();
    let db_save = Arc::clone(&state.db);
    let edit_doc_id = edit.document_id;
    let reset = reset_unlocked.unwrap_or(true);
    let overwrite = overwrite_query_fields.unwrap_or(false);
    let (blocked_by_verified, guided_hints) =
        tokio::task::spawn_blocking(move || -> Result<(bool, Option<GuidedSearchHints>), String> {
            let verified = {
                let r = db_save.reader();
                is_document_metadata_verified(&r, edit_doc_id).map_err(|e| e.to_string())?
            };
            if verified && reset {
                apply_manual_metadata_edit_with_locks(
                    &db_save,
                    &edit,
                    locked_fields.as_deref(),
                    false,
                )
                .map_err(|e| e.to_string())?;
                return Ok((true, None));
            }
            if overwrite && reset {
                let effective_locks: Vec<String> = locked_fields
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| f.trim().to_string())
                    .filter(|f| !f.is_empty())
                    .collect();
                let hints = prepare_guided_search_edit(&db_save, &edit, &effective_locks)
                    .map_err(|e| e.to_string())?;
                return Ok((false, Some(hints)));
            }
            apply_manual_metadata_edit_with_locks(&db_save, &edit, locked_fields.as_deref(), reset)
                .map_err(|e| e.to_string())?;
            Ok((false, None))
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
    let resolve_res = tokio::select! {
        biased;
        _ = &mut cancel_wait => {
            Ok(ResolveOutcome {
                document_id: edit_doc_id,
                status: "cancelled".to_string(),
                source: None,
                candidates_count: 0,
            })
        }
        r = resolve_document_metadata_with_hints(
            &state.db,
            &clients,
            &cfg,
            edit_doc_id,
            MetadataLookupScope::SingleRecordUserAction,
            guided_hints,
        ) => r,
    };
    let outcome = resolve_res.map_err(|e| e.to_string())?;

    let db_after = Arc::clone(&state.db);
    let metadata = tokio::task::spawn_blocking(move || {
        core_get_document_metadata(&db_after, edit_doc_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    if outcome.status != "cancelled" {
        spawn_background_dedup(&state);
        state.schedule_bibtex_auto_sync();
    }
    state.schedule_auto_backup();
    Ok(SaveAndFetchResult { outcome, metadata })
}

#[tauri::command]
pub async fn fetch_document_metadata(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
) -> Result<ResolveOutcome, String> {
    let cancel_wait = state.single_metadata_cancel_notify.notified();
    tokio::pin!(cancel_wait);
    cancel_wait.as_mut().enable();
    let cfg = state.config.read().expect("config poisoned").clone();
    let clients = Arc::clone(&state.clients.read().expect("clients poisoned"));
    let resolve_res = tokio::select! {
        biased;
        _ = &mut cancel_wait => {
            Ok(ResolveOutcome {
                document_id,
                status: "cancelled".to_string(),
                source: None,
                candidates_count: 0,
            })
        }
        r = resolve_document_metadata(
            &state.db,
            &clients,
            &cfg,
            document_id,
            MetadataLookupScope::SingleRecordUserAction,
        ) => r,
    };
    let outcome = resolve_res.map_err(|e| e.to_string())?;
    if outcome.status == "updated" || outcome.status == "candidates_found" {
        spawn_background_dedup(&state);
        state.schedule_auto_backup();
        if outcome.status == "updated" {
            state.schedule_bibtex_auto_sync();
        }
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
            "Ricerche metadati online disabilitate nelle Impostazioni -> Metadati & Rete."
                .to_string(),
        );
    }

    state.bg_metadata_cancel.store(true, Ordering::SeqCst);
    state.metadata_cancel_notify.notify_waiters();

    let _embed_pause = AppState::pause_embed_for_batch(state.inner());
    let _meta_guard = state.metadata_run_mutex.lock().await;
    let _active_guard = AppState::mark_metadata_batch_active(state.inner());

    let clients = Arc::clone(&state.clients.read().expect("clients poisoned"));
    state.metadata_batch_cancel.store(false, Ordering::SeqCst);

    let db_list = Arc::clone(&state.db);
    let unresolved_flag = only_unresolved.unwrap_or(true);
    let docs = tokio::task::spawn_blocking(move || -> Result<Vec<(i64, String)>, String> {
        let r = db_list.reader();
        let mut items =
            list_documents_for_bulk_metadata(&r, unresolved_flag).map_err(|e| e.to_string())?;
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
        let cancel_wait = state.metadata_cancel_notify.notified();
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

        let resolve_res = tokio::select! {
            biased;
            _ = cancel_wait => {
                cancelled = true;
                break;
            }
            r = resolve_document_metadata(
                &state.db,
                &clients,
                &cfg,
                doc_id,
                MetadataLookupScope::BulkBackgroundScan,
            ) => r,
        };

        match resolve_res {
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

    drop(_active_guard);
    drop(_meta_guard);
    drop(_embed_pause);

    if updated > 0 {
        let st_dedup = Arc::clone(&state);
        tauri::async_runtime::spawn(async move {
            let db_dedup = Arc::clone(&st_dedup.db);
            let _ = tokio::task::spawn_blocking(move || run_duplicate_detection(&db_dedup)).await;
            if let Ok(guard) = st_dedup.app_handle.lock() {
                if let Some(app) = guard.as_ref() {
                    let _ = app.emit("library-fs-updated", ());
                }
            }
        });
    }
    if updated > 0 || candidates > 0 {
        state.schedule_auto_backup();
        if updated > 0 {
            state.schedule_bibtex_auto_sync();
        }
    }
    if !cancelled {
        spawn_background_embedder(Arc::clone(&state));
    }

    let summary = BulkMetadataProgressEvent {
        current: if cancelled {
            updated + candidates + unchanged + failed
        } else {
            total
        },
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
    state.bg_metadata_cancel.store(true, Ordering::SeqCst);
    state.metadata_cancel_notify.notify_waiters();
    Ok(())
}

#[tauri::command]
pub async fn confirm_candidate(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    candidate_id: i64,
    overwrite_query_fields: Option<bool>,
) -> Result<DocumentMetadataView, String> {
    let db = Arc::clone(&state.db);
    let overwrite = overwrite_query_fields.unwrap_or(false);
    let view = tokio::task::spawn_blocking(move || {
        confirm_metadata_candidate_with_options(&db, document_id, candidate_id, overwrite)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    spawn_background_dedup(&state);
    state.schedule_auto_backup();
    state.schedule_bibtex_auto_sync();
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
pub async fn import_metadata_file(
    state: State<'_, Arc<AppState>>,
    path: Option<String>,
    content: Option<String>,
    format: Option<String>,
    target_document_id: Option<i64>,
) -> Result<ImportMetadataReport, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
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
        let matched_documents =
            import_metadata_records_into_library(&st.db, &records, target_document_id)
                .map_err(|e| e.to_string())?;
        let _ = run_duplicate_detection(&st.db);
        if matched_documents > 0 {
            st.schedule_auto_backup();
            st.schedule_bibtex_auto_sync();
        }
        Ok(ImportMetadataReport {
            total_entries,
            matched_documents,
            unmatched_entries: total_entries.saturating_sub(matched_documents),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_duplicates(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<DuplicateGroupView>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || list_duplicate_groups(&db).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn run_dedup_scan(state: State<'_, Arc<AppState>>) -> Result<DuplicateReport, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || run_duplicate_detection(&db).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn merge_duplicates(
    state: State<'_, Arc<AppState>>,
    group_id: i64,
    keep_document_id: i64,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        merge_duplicate_group(&db, group_id, keep_document_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn resolve_duplicate(
    state: State<'_, Arc<AppState>>,
    group_id: i64,
    decision: String,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        resolve_duplicate_group(&db, group_id, &decision).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn get_document_relations_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
) -> Result<DocumentRelationsView, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        {
            let w = st.db.writer();
            let _ = refresh_local_citations_for_doc(&w, document_id);
        }
        let raw_prof = st.config.read().expect("config poisoned").ml_profile.clone();
        let profile = ModelProfile::from_str_lossy(&raw_prof);
        let vindex = st.vector_index_for(profile);
        let r = st.db.reader();
        get_document_relations(&r, Some(vindex.as_ref()), document_id, 5).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn match_citation_in_library_cmd(
    state: State<'_, Arc<AppState>>,
    citing_document_id: Option<i64>,
    snippet: String,
) -> Result<Option<CitationLinkItem>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        resolve_citation_snippet_in_library(&r, citing_document_id, &snippet)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_missing_classics_cmd(
    state: State<'_, Arc<AppState>>,
    category: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<MissingClassicItem>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        find_missing_cited_classics(&r, category.as_deref(), limit.unwrap_or(50))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn apply_smart_paste_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    raw_input: String,
) -> Result<SmartPasteApplyResult, String> {
    let parsed = parse_smart_paste(&raw_input);
    match parsed {
        SmartPasteResult::Empty => {
            Err("Nessun DOI, arXiv, MR, zbMATH, ISBN o voce BibTeX riconosciuto.".to_string())
        }
        SmartPasteResult::Bibtex { records } => {
            let db = Arc::clone(&state.db);
            let metadata = tokio::task::spawn_blocking(move || -> Result<DocumentMetadataView, String> {
                import_metadata_records_into_library(&db, &records[..1], Some(document_id))
                    .map_err(|e| e.to_string())?;
                core_get_document_metadata(&db, document_id).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            spawn_background_dedup(&state);
            state.schedule_auto_backup();
            state.schedule_bibtex_auto_sync();
            Ok(SmartPasteApplyResult {
                kind: "bibtex".to_string(),
                summary: "BibTeX importato e applicato.".to_string(),
                outcome: None,
                metadata,
            })
        }
        SmartPasteResult::Identifiers {
            doi,
            arxiv_id,
            mr_number,
            zbl_number,
            isbn,
        } => {
            let mut summary_parts = Vec::new();
            if let Some(ref d) = doi {
                summary_parts.push(format!("DOI {d}"));
            }
            if let Some(ref a) = arxiv_id {
                summary_parts.push(format!("arXiv:{a}"));
            }
            if let Some(ref m) = mr_number {
                summary_parts.push(format!("MR{m}"));
            }
            if let Some(ref z) = zbl_number {
                summary_parts.push(format!("Zbl {z}"));
            }
            if let Some(ref i) = isbn {
                summary_parts.push(format!("ISBN {i}"));
            }
            let summary = summary_parts.join(" · ");

            let db_upd = Arc::clone(&state.db);
            let (d_c, a_c, m_c, z_c, i_c) = (
                doi.clone(),
                arxiv_id.clone(),
                mr_number.clone(),
                zbl_number.clone(),
                isbn.clone(),
            );
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let current =
                    core_get_document_metadata(&db_upd, document_id).map_err(|e| e.to_string())?;
                let mut locks: Vec<String> = current.manual_locked_fields.clone();
                let mut push_lock = |name: &str| {
                    if !locks.iter().any(|l| l == name) {
                        locks.push(name.to_string());
                    }
                };
                if d_c.is_some() {
                    push_lock("doi");
                }
                if a_c.is_some() {
                    push_lock("arxiv_id");
                }
                if m_c.is_some() {
                    push_lock("mr_number");
                }
                if z_c.is_some() {
                    push_lock("zbl_number");
                }
                if i_c.is_some() {
                    push_lock("isbn");
                }
                let edit = ManualMetadataEdit {
                    document_id,
                    work_type: current.work_type,
                    title: current.title,
                    authors: current.authors,
                    journal_abbrev: current.journal_abbrev,
                    journal_full: current.journal_full,
                    booktitle: current.booktitle,
                    series: current.series,
                    volume: current.volume,
                    issue_number: current.issue_number,
                    year: current.year,
                    pages: current.pages,
                    edition: current.edition,
                    publisher: current.publisher,
                    address: current.address,
                    isbn: i_c.or(current.isbn),
                    issn: current.issn,
                    doi: d_c.or(current.doi),
                    arxiv_id: a_c.or(current.arxiv_id),
                    mr_number: m_c.or(current.mr_number),
                    mr_reviewer: current.mr_reviewer,
                    zbl_number: z_c.or(current.zbl_number),
                    url: current.url,
                    msc_primary: current.msc_primary,
                    msc_secondary: current.msc_secondary,
                    abstract_text: current.abstract_text,
                    citation_key_override: current.citation_key_override,
                    exclude_from_cloud_llm: current.exclude_from_cloud_llm,
                    tags: current.tags,
                    edited_fields: locks.clone(),
                };
                apply_manual_metadata_edit_with_locks(&db_upd, &edit, Some(&locks), false)
                    .map_err(|e| e.to_string())?;
                Ok(())
            })
            .await
            .map_err(|e| e.to_string())??;

            let cfg = state.config.read().expect("config poisoned").clone();
            let outcome = if cfg.network_lookups_enabled {
                let clients = Arc::clone(&state.clients.read().expect("clients poisoned"));
                let res = resolve_document_metadata(
                    &state.db,
                    &clients,
                    &cfg,
                    document_id,
                    MetadataLookupScope::SingleRecordUserAction,
                )
                .await
                .map_err(|e| e.to_string())?;
                Some(res)
            } else {
                None
            };

            let db_after = Arc::clone(&state.db);
            let metadata = tokio::task::spawn_blocking(move || {
                core_get_document_metadata(&db_after, document_id).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;

            spawn_background_dedup(&state);
            state.schedule_auto_backup();
            state.schedule_bibtex_auto_sync();

            Ok(SmartPasteApplyResult {
                kind: "identifiers".to_string(),
                summary,
                outcome,
                metadata,
            })
        }
    }
}
