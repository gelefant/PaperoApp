use crate::state::AppState;
use papero_core::bibtex::BibtexExportOptions;
use papero_core::db::repos::collections::{list_collections, set_collection_bib_sync, CollectionView};
use papero_core::db::repos::roots::list_source_roots;
use papero_core::metadata::io::{
    export_documents_bibtex, export_documents_csv, resolve_disambiguated_cite_key_for_doc,
    write_bibtex_file_atomically,
};
use papero_core::metadata::models::DocumentMetadataView;
use papero_core::metadata::resolver::{get_documents_metadata_for_export, list_all_document_ids};
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use tauri::State;

pub(crate) fn current_bibtex_options(state: &AppState) -> BibtexExportOptions {
    let cfg = state.config.read().expect("config poisoned");
    BibtexExportOptions {
        key_pattern: cfg.bibtex_key_pattern,
        format: cfg.bibtex_format,
        ..Default::default()
    }
}

pub(crate) fn is_global_bibtex_sync_active(state: &AppState) -> bool {
    let cfg = state.config.read().expect("config poisoned");
    cfg.bibtex_auto_sync_enabled
        && cfg
            .bibtex_auto_sync_path
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty())
}

pub(crate) fn load_source_root_paths(state: &AppState) -> Vec<String> {
    let r = state.db.reader();
    list_source_roots(&r)
        .unwrap_or_default()
        .into_iter()
        .map(|sr| sr.root_path)
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct CiteKeyPayload {
    pub key: String,
    pub cite_command: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BibtexSyncReport {
    pub path: String,
    pub count: usize,
}

#[tauri::command]
pub async fn get_document_bibtex(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    collection_id: Option<i64>,
) -> Result<String, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let opts = current_bibtex_options(&st);
        let global_sync = is_global_bibtex_sync_active(&st);
        let r = st.db.reader();
        let (_key, bib) = resolve_disambiguated_cite_key_for_doc(
            &r,
            document_id,
            collection_id,
            global_sync,
            &opts,
        )
        .map_err(|e| e.to_string())?;
        Ok(bib)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_document_cite_key_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    collection_id: Option<i64>,
) -> Result<CiteKeyPayload, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let opts = current_bibtex_options(&st);
        let global_sync = is_global_bibtex_sync_active(&st);
        let r = st.db.reader();
        let (key, _bib) = resolve_disambiguated_cite_key_for_doc(
            &r,
            document_id,
            collection_id,
            global_sync,
            &opts,
        )
        .map_err(|e| e.to_string())?;
        let cite_command = format!("\\cite{{{key}}}");
        Ok(CiteKeyPayload { key, cite_command })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn sync_bibtex_now_cmd(
    state: State<'_, Arc<AppState>>,
) -> Result<BibtexSyncReport, String> {
    let (path, count) = AppState::run_bibtex_auto_sync_now(&state).await?;
    Ok(BibtexSyncReport { path, count })
}

fn load_export_views(
    state: &AppState,
    document_ids: &[i64],
) -> Result<Vec<DocumentMetadataView>, String> {
    let r = state.db.reader();
    let ids = if document_ids.is_empty() {
        list_all_document_ids(&r).map_err(|e| e.to_string())?
    } else {
        document_ids.to_vec()
    };
    get_documents_metadata_for_export(&r, &ids).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn export_bibtex_many(
    state: State<'_, Arc<AppState>>,
    document_ids: Vec<i64>,
    file_path: Option<String>,
) -> Result<String, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let opts = current_bibtex_options(&st);
        let views = load_export_views(&st, &document_ids)?;
        let out = export_documents_bibtex(&views, &opts);
        if let Some(p) = file_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let roots = load_source_root_paths(&st);
            write_bibtex_file_atomically(Path::new(p), &out, &roots).map_err(|e| e.to_string())?;
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn export_csv_many(
    state: State<'_, Arc<AppState>>,
    document_ids: Vec<i64>,
    file_path: Option<String>,
) -> Result<String, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let views = load_export_views(&st, &document_ids)?;
        let out = export_documents_csv(&views);
        if let Some(p) = file_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let roots = load_source_root_paths(&st);
            write_bibtex_file_atomically(Path::new(p), &out, &roots).map_err(|e| e.to_string())?;
        }
        Ok(out)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_collection_bib_sync_cmd(
    state: State<'_, Arc<AppState>>,
    collection_id: i64,
    bib_sync_path: Option<String>,
    bib_sync_enabled: bool,
) -> Result<CollectionView, String> {
    let st = Arc::clone(&state);
    let clean_path = bib_sync_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    if bib_sync_enabled {
        if let Some(ref target) = clean_path {
            let db_check = Arc::clone(&st.db);
            let cfg = st.config.read().expect("config poisoned").clone();
            let target_owned = target.clone();
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let (roots, doc_ids) = {
                    let r = db_check.reader();
                    let col = list_collections(&r)
                        .map_err(|e| e.to_string())?
                        .into_iter()
                        .find(|c| c.id == collection_id)
                        .ok_or_else(|| format!("Collezione {collection_id} non trovata."))?;
                    let roots = list_source_roots(&r).unwrap_or_default();
                    (roots, col.document_ids)
                };
                let views = {
                    let r = db_check.reader();
                    if doc_ids.is_empty() {
                        Vec::new()
                    } else {
                        get_documents_metadata_for_export(&r, &doc_ids)
                            .map_err(|e| e.to_string())?
                    }
                };
                let root_paths: Vec<String> = roots.into_iter().map(|sr| sr.root_path).collect();
                let opts = BibtexExportOptions {
                    key_pattern: cfg.bibtex_key_pattern,
                    format: cfg.bibtex_format,
                    ..Default::default()
                };
                let bibtex = export_documents_bibtex(&views, &opts);
                write_bibtex_file_atomically(Path::new(&target_owned), &bibtex, &root_paths)
                    .map_err(|e| e.to_string())?;
                Ok(())
            })
            .await
            .map_err(|e| e.to_string())??;
        }
    }

    let db = Arc::clone(&state.db);
    let updated = tokio::task::spawn_blocking(move || {
        let w = db.writer();
        set_collection_bib_sync(&w, collection_id, clean_path.as_deref(), bib_sync_enabled)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(updated)
}

#[tauri::command]
pub async fn sync_collection_bibtex_now_cmd(
    state: State<'_, Arc<AppState>>,
    collection_id: i64,
) -> Result<(String, usize), String> {
    AppState::run_collection_bibtex_sync_now(state.inner(), collection_id).await
}
