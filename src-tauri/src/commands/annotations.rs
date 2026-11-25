use crate::commands::bibtex::load_source_root_paths;
use crate::state::AppState;
use papero_core::db::repos::annotations::{
    count_total_annotations, create_annotation_for_file, delete_annotation,
    export_annotations_markdown, list_annotations_for_file, search_annotations,
    set_document_reading_status, set_document_starred, toggle_page_bookmark, update_annotation,
    upsert_reading_state_for_file, AnnotationSearchHit, FileAnnotation, FileReadingState,
    NewFileAnnotation, ReadingStatePatch, UpdateFileAnnotation,
};
use papero_core::metadata::io::write_bibtex_file_atomically;
use std::path::Path;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn list_file_annotations_cmd(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
) -> Result<Vec<FileAnnotation>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        list_annotations_for_file(&r, file_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn create_file_annotation_cmd(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    input: NewFileAnnotation,
) -> Result<FileAnnotation, String> {
    let db = Arc::clone(&state.db);
    let ann = tokio::task::spawn_blocking(move || {
        let w = db.writer();
        create_annotation_for_file(&w, file_id, &input).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(ann)
}

#[tauri::command]
pub async fn update_file_annotation_cmd(
    state: State<'_, Arc<AppState>>,
    annotation_id: i64,
    patch: UpdateFileAnnotation,
) -> Result<FileAnnotation, String> {
    let db = Arc::clone(&state.db);
    let ann = tokio::task::spawn_blocking(move || {
        let w = db.writer();
        update_annotation(&w, annotation_id, &patch).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(ann)
}

#[tauri::command]
pub async fn delete_file_annotation_cmd(
    state: State<'_, Arc<AppState>>,
    annotation_id: i64,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        delete_annotation(&w, annotation_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn toggle_page_bookmark_cmd(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    page_ord: u32,
    label: Option<String>,
) -> Result<Option<FileAnnotation>, String> {
    let db = Arc::clone(&state.db);
    let res = tokio::task::spawn_blocking(move || {
        let w = db.writer();
        toggle_page_bookmark(&w, file_id, page_ord, label.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(res)
}

#[tauri::command]
pub async fn search_annotations_cmd(
    state: State<'_, Arc<AppState>>,
    query: String,
    file_id: Option<i64>,
) -> Result<Vec<AnnotationSearchHit>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        search_annotations(&r, &query, file_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn export_annotations_markdown_cmd(
    state: State<'_, Arc<AppState>>,
    file_id: Option<i64>,
    file_path: Option<String>,
) -> Result<String, String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let md = {
            let r = st.db.reader();
            export_annotations_markdown(&r, file_id).map_err(|e| e.to_string())?
        };
        if let Some(p) = file_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let roots = load_source_root_paths(&st);
            write_bibtex_file_atomically(Path::new(p), &md, &roots).map_err(|e| e.to_string())?;
        }
        Ok(md)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn count_total_annotations_cmd(
    state: State<'_, Arc<AppState>>,
) -> Result<usize, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        count_total_annotations(&r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn upsert_reading_state_cmd(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    patch: ReadingStatePatch,
) -> Result<FileReadingState, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        upsert_reading_state_for_file(&w, file_id, &patch).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn set_document_reading_status_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    status: String,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        set_document_reading_status(&w, document_id, &status).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn set_document_starred_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    starred: bool,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        set_document_starred(&w, document_id, starred).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}
