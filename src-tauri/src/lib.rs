mod commands;
mod protocol;
mod state;

use crate::protocol::handle_papero_doc_request;
use crate::state::AppState;
use std::sync::Arc;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = Arc::new(AppState::initialize().expect("failed to initialize Papero state"));
    let protocol_state = Arc::clone(&app_state);
    let reaper_state = Arc::clone(&app_state);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .setup(move |app| {
            AppState::spawn_idle_reaper(Arc::clone(&reaper_state));
            AppState::spawn_auto_backup_loop(Arc::clone(&reaper_state));
            AppState::refresh_watcher(&reaper_state, Some(app.handle().clone()));
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
            commands::confirm_candidate,
            commands::dismiss_candidate,
            commands::get_document_bibtex,
            commands::export_bibtex_many,
            commands::export_csv_many,
            commands::import_metadata_file,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
