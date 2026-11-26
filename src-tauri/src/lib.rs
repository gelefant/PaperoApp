mod commands;
mod protocol;
mod state;

use crate::protocol::handle_papero_doc_request;
use crate::state::AppState;
use std::sync::Arc;

#[cfg(target_os = "windows")]
fn show_windows_error_dialog(title: &str, body: &str) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(
            hwnd: *mut std::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            utype: u32,
        ) -> i32;
    }
    const MB_OK: u32 = 0x0000_0000;
    const MB_ICONERROR: u32 = 0x0000_0010;
    let wide_title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let wide_body: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: Both UTF-16 slices are NUL-terminated and live for the duration of `MessageBoxW`.
    unsafe {
        let _ = MessageBoxW(
            std::ptr::null_mut(),
            wide_body.as_ptr(),
            wide_title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn startup_crash_log_path() -> std::path::PathBuf {
    let log_dir = papero_core::config::AppPaths::default_app_dir().join("logs");
    if std::fs::create_dir_all(&log_dir).is_ok() {
        log_dir.join("startup-error.log")
    } else {
        std::env::temp_dir().join("papero-startup-error.log")
    }
}

fn report_fatal_startup_error(context: &str, detail: &str) -> ! {
    tracing::error!("{context}: {detail}");
    eprintln!("Papero fatal error ({context}): {detail}");

    let crash_path = startup_crash_log_path();
    let _ = std::fs::write(&crash_path, format!("{context}\n\n{detail}\n"));

    #[cfg(target_os = "windows")]
    {
        let dialog_msg = format!(
            "{context}:\n\n{detail}\n\nDettagli salvati in:\n{}",
            crash_path.display()
        );
        show_windows_error_dialog("Papero — Errore di avvio", &dialog_msg);
    }

    std::process::exit(1);
}

#[cfg(target_os = "windows")]
mod win_seh_logger {
    use std::ffi::c_void;
    use std::io::Write as _;

    #[repr(C)]
    struct ExceptionRecord {
        exception_code: u32,
        exception_flags: u32,
        exception_record: *mut ExceptionRecord,
        exception_address: *mut c_void,
        number_parameters: u32,
        exception_information: [usize; 15],
    }

    #[repr(C)]
    struct ExceptionPointers {
        exception_record: *mut ExceptionRecord,
        context_record: *mut c_void,
    }

    type TopLevelExceptionFilter =
        Option<unsafe extern "system" fn(*const ExceptionPointers) -> i32>;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetUnhandledExceptionFilter(
            lp_top_level_exception_filter: TopLevelExceptionFilter,
        ) -> TopLevelExceptionFilter;
        fn GetModuleHandleExW(
            dw_flags: u32,
            lp_module_name: *const u16,
            ph_module: *mut *mut c_void,
        ) -> i32;
        fn GetModuleFileNameW(
            h_module: *mut c_void,
            lp_filename: *mut u16,
            n_size: u32,
        ) -> u32;
    }

    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    const STATUS_STACK_OVERFLOW: u32 = 0xC000_00FD;
    const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x0000_0002;
    const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x0000_0004;

    unsafe extern "system" fn unhandled_seh_handler(info: *const ExceptionPointers) -> i32 {
        if !info.is_null() {
            let rec = (*info).exception_record;
            if !rec.is_null() {
                let code = (*rec).exception_code;
                let addr = (*rec).exception_address;
                if code != STATUS_STACK_OVERFLOW {
                    let mut hmod: *mut c_void = std::ptr::null_mut();
                    let mut mod_name = String::from("<unknown>");
                    if GetModuleHandleExW(
                        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                            | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                        addr as *const u16,
                        &mut hmod,
                    ) != 0
                        && !hmod.is_null()
                    {
                        let mut buf = [0u16; 512];
                        let len =
                            GetModuleFileNameW(hmod, buf.as_mut_ptr(), buf.len() as u32) as usize;
                        if len > 0 && len <= buf.len() {
                            mod_name = String::from_utf16_lossy(&buf[..len]);
                        }
                    }
                    let crash_path = super::startup_crash_log_path();
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&crash_path)
                    {
                        let _ = writeln!(
                            file,
                            "win32_seh_crash: code=0x{code:08X} addr={addr:p} module={mod_name}"
                        );
                    }
                }
            }
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    pub fn install() {
        // SAFETY: `unhandled_seh_handler` is a valid `"system"` callback with static lifetime.
        unsafe {
            let _ = SetUnhandledExceptionFilter(Some(unhandled_seh_handler));
        }
    }
}

fn install_panic_logger() {
    #[cfg(target_os = "windows")]
    win_seh_logger::install();

    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread_name = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        let msg = format!("panic [thread={thread_name}]: {info}");
        tracing::error!("{msg}");
        let crash_path = startup_crash_log_path();
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&crash_path)
        {
            use std::io::Write as _;
            let _ = writeln!(file, "{msg}");
        }
        prev_hook(info);
    }));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_panic_logger();

    let app_state = match AppState::initialize() {
        Ok(state) => Arc::new(state),
        Err(err) => {
            report_fatal_startup_error("Impossibile inizializzare lo stato di Papero", &err.to_string())
        }
    };
    let protocol_state = Arc::clone(&app_state);
    let reaper_state = Arc::clone(&app_state);

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .setup(move |app| {
            AppState::spawn_idle_reaper(Arc::clone(&reaper_state));
            AppState::spawn_auto_backup_loop(Arc::clone(&reaper_state));
            AppState::spawn_bibtex_auto_sync_loop(Arc::clone(&reaper_state));
            AppState::refresh_watcher(&reaper_state, Some(app.handle().clone()));
            AppState::spawn_startup_pending_indexing(Arc::clone(&reaper_state));
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("papero-doc", move |_ctx, request, responder| {
            let st = Arc::clone(&protocol_state);
            tauri::async_runtime::spawn_blocking(move || {
                let resp = handle_papero_doc_request(st, request);
                responder.respond(resp);
            });
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_roots,
            commands::add_source_root,
            commands::rebind_source_root,
            commands::toggle_source_root,
            commands::delete_source_root,
            commands::trigger_rescan,
            commands::control_rescan,
            commands::list_documents,
            commands::search_content_bm25,
            commands::search_content_hybrid,
            commands::list_model_statuses,
            commands::download_ml_profile,
            commands::unload_ml_models,
            commands::import_alt_formula_text,
            commands::get_viewer_doc,
            commands::read_pdf_range_cmd,
            commands::render_djvu_page_cmd,
            commands::get_djvu_page_text,
            commands::open_file_external,
            commands::reveal_file_in_folder,
            commands::list_external_presets,
            commands::get_ui_settings,
            commands::save_ui_settings,
            commands::open_detached_viewer_window,
            commands::search_metadata,
            commands::get_document_metadata,
            commands::set_document_metadata_verified_cmd,
            commands::save_document_metadata,
            commands::save_and_fetch_document_metadata,
            commands::fetch_document_metadata,
            commands::fetch_all_metadata_cmd,
            commands::cancel_metadata_batch_cmd,
            commands::cancel_single_metadata_cmd,
            commands::get_embed_progress_cmd,
            commands::set_embed_paused_cmd,
            commands::confirm_candidate,
            commands::dismiss_candidate,
            commands::get_document_bibtex,
            commands::get_document_cite_key_cmd,
            commands::sync_bibtex_now_cmd,
            commands::export_bibtex_many,
            commands::export_csv_many,
            commands::import_metadata_file,
            commands::apply_smart_paste_cmd,
            commands::get_document_relations_cmd,
            commands::match_citation_in_library_cmd,
            commands::list_file_annotations_cmd,
            commands::create_file_annotation_cmd,
            commands::update_file_annotation_cmd,
            commands::delete_file_annotation_cmd,
            commands::toggle_page_bookmark_cmd,
            commands::search_annotations_cmd,
            commands::export_annotations_markdown_cmd,
            commands::upsert_reading_state_cmd,
            commands::set_document_reading_status_cmd,
            commands::set_document_starred_cmd,
            commands::list_collections_cmd,
            commands::create_collection_cmd,
            commands::rename_collection_cmd,
            commands::delete_collection_cmd,
            commands::reorder_collections_cmd,
            commands::add_docs_to_collection_cmd,
            commands::remove_docs_from_collection_cmd,
            commands::list_saved_searches_cmd,
            commands::create_saved_search_cmd,
            commands::delete_saved_search_cmd,
            commands::batch_add_tag_cmd,
            commands::batch_remove_tag_cmd,
            commands::batch_set_verified_cmd,
            commands::batch_enqueue_ocr_cmd,
            commands::list_duplicates,
            commands::run_dedup_scan,
            commands::merge_duplicates,
            commands::resolve_duplicate,
            commands::get_msc_tree,
            commands::run_knn_msc_proposals,
            commands::confirm_msc_classification,
            commands::reject_msc_proposal,
            commands::run_topic_clustering_cmd,
            commands::list_topic_clusters,
            commands::rename_topic_cluster_cmd,
            commands::set_document_cluster_cmd,
            commands::remove_document_cluster_cmd,
            commands::list_all_tags_cmd,
            commands::add_document_tag_cmd,
            commands::remove_document_tag_cmd,
            commands::create_backup_cmd,
            commands::update_backup_now_cmd,
            commands::restore_backup_cmd,
            commands::get_ocr_status,
            commands::run_ocr_for_file_cmd,
            commands::run_pending_ocr_cmd,
            commands::set_root_cloud_exclusion,
            commands::get_llm_key_status,
            commands::save_llm_api_key,
            commands::remove_llm_api_key,
            commands::verify_llm_key_cmd,
            commands::list_llm_models_cmd,
            commands::preview_llm_prompt_cmd,
            commands::accept_llm_cloud_consent_cmd,
            commands::generate_llm_answer_cmd,
            commands::generate_llm_answer_stream_cmd,
            commands::preview_doc_llm_prompt_cmd,
            commands::ask_document_llm_stream_cmd,
            commands::snip_image_to_latex_cmd,
            commands::count_total_annotations_cmd,
            commands::set_collection_bib_sync_cmd,
            commands::sync_collection_bibtex_now_cmd,
            commands::get_missing_classics_cmd,
            commands::get_storage_health_cmd,
            commands::compact_database_cmd,
            commands::clear_render_cache_cmd,
            commands::check_app_update_cmd,
            commands::open_external_url_cmd,
        ])
        .run(tauri::generate_context!());

    if let Err(err) = result {
        report_fatal_startup_error("Errore durante l'avvio della finestra di Papero", &err.to_string());
    }
}
