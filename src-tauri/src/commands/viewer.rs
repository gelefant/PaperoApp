use crate::state::{read_file_range_at, AppState, ViewerFileError};
use papero_core::db::repos::docs::{get_file_location, get_file_viewer_info, FileViewerInfo};
use papero_core::extract::djvu::extract_djvu_single_page_spans;
use papero_core::extract::types::ExtractedSpan;
use papero_core::viewer::os_actions::{
    external_reader_presets, open_external, reveal_in_folder, ExternalReaderPreset,
};
use std::sync::Arc;
use tauri::ipc::Response as IpcResponse;
use tauri::{AppHandle, State, WebviewUrl, WebviewWindowBuilder};

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

fn viewer_file_err_to_string(file_id: i64, err: ViewerFileError) -> String {
    match err {
        ViewerFileError::NotFound => format!("file {file_id} not found"),
        ViewerFileError::Unreachable => "Cartella sorgente non raggiungibile.".to_string(),
        ViewerFileError::Forbidden(m) | ViewerFileError::Io(m) => m,
    }
}

#[tauri::command]
pub async fn read_pdf_range_cmd(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    start: u64,
    length: usize,
) -> Result<IpcResponse, String> {
    let st = Arc::clone(&state);
    let buf = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let cached = st
            .get_or_open_viewer_file(file_id)
            .map_err(|e| viewer_file_err_to_string(file_id, e))?;
        if start >= cached.len || length == 0 {
            return Ok(Vec::new());
        }
        let capped_len = (length as u64)
            .min(cached.len - start)
            .min(4 * 1024 * 1024) as usize;
        read_file_range_at(&cached.file, start, capped_len).map_err(|e| e.to_string())
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
        let info = st
            .get_viewer_stream_info(file_id)
            .map_err(|e| viewer_file_err_to_string(file_id, e))?;
        let ddjvu_bin = st.config.read().expect("config poisoned").ddjvu_bin.clone();
        st.djvu_cache
            .render_page_png(
                &info.abs_path,
                &info.blake3_hash,
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
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let info = st
            .get_viewer_stream_info(file_id)
            .map_err(|e| viewer_file_err_to_string(file_id, e))?;
        let djvused_bin = st
            .config
            .read()
            .expect("config poisoned")
            .djvused_bin
            .clone();
        extract_djvu_single_page_spans(&info.abs_path, page_ord, djvused_bin.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn open_file_external(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
    page_ord: Option<u32>,
    template_override: Option<String>,
) -> Result<(), String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let loc = {
            let r = st.db.reader();
            get_file_location(&r, file_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("file {file_id} not found"))?
        };
        if !loc.is_available || !loc.abs_path.exists() {
            return Err("Cartella sorgente non raggiungibile".into());
        }
        let cfg = st.config.read().expect("config poisoned");
        let configured_tpl = match loc.format.as_str() {
            "djvu" => cfg.external_djvu_command.as_deref(),
            _ => cfg.external_pdf_command.as_deref(),
        };
        let effective_tpl = template_override.as_deref().or(configured_tpl);
        open_external(&loc.abs_path, page_ord, effective_tpl).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn reveal_file_in_folder(
    state: State<'_, Arc<AppState>>,
    file_id: i64,
) -> Result<(), String> {
    let st = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let loc = {
            let r = st.db.reader();
            get_file_location(&r, file_id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("file {file_id} not found"))?
        };
        if !loc.is_available || !loc.abs_path.exists() {
            return Err("Cartella sorgente non raggiungibile".into());
        }
        let mode = st.config.read().expect("config poisoned").linux_reveal_mode;
        reveal_in_folder(&loc.abs_path, mode).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn list_external_presets() -> Vec<ExternalReaderPreset> {
    external_reader_presets()
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
        let encoded: String = b.bytes().map(|byte| format!("%{byte:02X}")).collect();
        url.push_str(&format!("&bboxes={encoded}"));
    }
    WebviewWindowBuilder::new(&app, label, WebviewUrl::App(url.into()))
        .title(format!("{title} — Papero"))
        .inner_size(1024.0, 820.0)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}
