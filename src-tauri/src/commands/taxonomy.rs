use crate::state::AppState;
use papero_core::classify::{
    add_tag_to_document, build_msc_tree, confirm_document_msc, list_all_tags,
    list_topic_clusters as core_list_topic_clusters, propose_msc_knn_on_db, reject_proposed_msc,
    remove_document_from_cluster, remove_tag_from_document, rename_topic_cluster,
    run_topic_clustering_on_db, set_document_cluster, ClusteringReport, KnnProposeReport,
    MscTreeNode, TagSummary, TopicClusterView,
};
use papero_core::db::repos::collections::{
    add_documents_to_collection, create_collection, create_saved_search, delete_collection,
    delete_saved_search, list_collections, list_saved_searches, remove_documents_from_collection,
    rename_collection, reorder_collections, CollectionView, SavedSearchView,
};
use papero_core::extract::ocr::enqueue_ocr_for_documents;
use papero_core::metadata::resolver::set_document_metadata_verified;
use papero_core::ml::ModelProfile;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn get_msc_tree(state: State<'_, Arc<AppState>>) -> Result<Vec<MscTreeNode>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        build_msc_tree(&r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
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
        propose_msc_knn_on_db(&st.db, &vindex, k.unwrap_or(7), 0.20).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if rep.proposed_count > 0 {
        state.schedule_auto_backup();
    }
    Ok(rep)
}

#[tauri::command]
pub async fn confirm_msc_classification(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    msc_code: Option<String>,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        confirm_document_msc(&w, document_id, msc_code.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn reject_msc_proposal(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        reject_proposed_msc(&w, document_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
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
        run_topic_clustering_on_db(&st.db, &vindex, profile, requested_k).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(rep)
}

#[tauri::command]
pub async fn list_topic_clusters(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<TopicClusterView>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        core_list_topic_clusters(&r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn rename_topic_cluster_cmd(
    state: State<'_, Arc<AppState>>,
    cluster_id: i64,
    label: String,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        rename_topic_cluster(&w, cluster_id, &label).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn set_document_cluster_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    cluster_id: i64,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        set_document_cluster(&w, document_id, cluster_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn remove_document_cluster_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    cluster_id: i64,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        remove_document_from_cluster(&w, document_id, cluster_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn list_all_tags_cmd(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<TagSummary>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        list_all_tags(&r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn add_document_tag_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    tag_name: String,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        add_tag_to_document(&w, document_id, &tag_name).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn remove_document_tag_cmd(
    state: State<'_, Arc<AppState>>,
    document_id: i64,
    tag_name: String,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        remove_tag_from_document(&w, document_id, &tag_name).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn list_collections_cmd(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<CollectionView>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        list_collections(&r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn create_collection_cmd(
    state: State<'_, Arc<AppState>>,
    name: String,
    color_hex: Option<String>,
) -> Result<CollectionView, String> {
    let db = Arc::clone(&state.db);
    let col = tokio::task::spawn_blocking(move || {
        let w = db.writer();
        create_collection(&w, &name, color_hex.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(col)
}

#[tauri::command]
pub async fn rename_collection_cmd(
    state: State<'_, Arc<AppState>>,
    collection_id: i64,
    name: String,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        rename_collection(&w, collection_id, &name).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn delete_collection_cmd(
    state: State<'_, Arc<AppState>>,
    collection_id: i64,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        delete_collection(&w, collection_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn reorder_collections_cmd(
    state: State<'_, Arc<AppState>>,
    ordered_ids: Vec<i64>,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        reorder_collections(&w, &ordered_ids).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn add_docs_to_collection_cmd(
    state: State<'_, Arc<AppState>>,
    collection_id: i64,
    document_ids: Vec<i64>,
) -> Result<usize, String> {
    let db = Arc::clone(&state.db);
    let n = tokio::task::spawn_blocking(move || {
        let w = db.writer();
        add_documents_to_collection(&w, collection_id, &document_ids).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if n > 0 {
        state.schedule_auto_backup();
        state.schedule_bibtex_auto_sync();
    }
    Ok(n)
}

#[tauri::command]
pub async fn remove_docs_from_collection_cmd(
    state: State<'_, Arc<AppState>>,
    collection_id: i64,
    document_ids: Vec<i64>,
) -> Result<usize, String> {
    let db = Arc::clone(&state.db);
    let n = tokio::task::spawn_blocking(move || {
        let w = db.writer();
        remove_documents_from_collection(&w, collection_id, &document_ids)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    if n > 0 {
        state.schedule_auto_backup();
        state.schedule_bibtex_auto_sync();
    }
    Ok(n)
}

#[tauri::command]
pub async fn list_saved_searches_cmd(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<SavedSearchView>, String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let r = db.reader();
        list_saved_searches(&r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn create_saved_search_cmd(
    state: State<'_, Arc<AppState>>,
    name: String,
    search_mode: String,
    query_text: String,
    filter_json: String,
) -> Result<SavedSearchView, String> {
    let db = Arc::clone(&state.db);
    let view = tokio::task::spawn_blocking(move || {
        let w = db.writer();
        create_saved_search(&w, &name, &search_mode, &query_text, &filter_json)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(view)
}

#[tauri::command]
pub async fn delete_saved_search_cmd(
    state: State<'_, Arc<AppState>>,
    saved_search_id: i64,
) -> Result<(), String> {
    let db = Arc::clone(&state.db);
    tokio::task::spawn_blocking(move || {
        let w = db.writer();
        delete_saved_search(&w, saved_search_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    state.schedule_auto_backup();
    Ok(())
}

#[tauri::command]
pub async fn batch_add_tag_cmd(
    state: State<'_, Arc<AppState>>,
    document_ids: Vec<i64>,
    tag_name: String,
) -> Result<usize, String> {
    let db = Arc::clone(&state.db);
    let count = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let w = db.writer();
        let mut applied = 0usize;
        for doc_id in document_ids {
            add_tag_to_document(&w, doc_id, &tag_name).map_err(|e| e.to_string())?;
            applied += 1;
        }
        Ok(applied)
    })
    .await
    .map_err(|e| e.to_string())??;
    if count > 0 {
        state.schedule_auto_backup();
    }
    Ok(count)
}

#[tauri::command]
pub async fn batch_remove_tag_cmd(
    state: State<'_, Arc<AppState>>,
    document_ids: Vec<i64>,
    tag_name: String,
) -> Result<usize, String> {
    let db = Arc::clone(&state.db);
    let count = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let w = db.writer();
        let mut removed = 0usize;
        for doc_id in document_ids {
            remove_tag_from_document(&w, doc_id, &tag_name).map_err(|e| e.to_string())?;
            removed += 1;
        }
        Ok(removed)
    })
    .await
    .map_err(|e| e.to_string())??;
    if count > 0 {
        state.schedule_auto_backup();
    }
    Ok(count)
}

#[tauri::command]
pub async fn batch_set_verified_cmd(
    state: State<'_, Arc<AppState>>,
    document_ids: Vec<i64>,
    verified: bool,
) -> Result<usize, String> {
    let db = Arc::clone(&state.db);
    let count = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let mut updated = 0usize;
        for doc_id in document_ids {
            set_document_metadata_verified(&db, doc_id, verified).map_err(|e| e.to_string())?;
            updated += 1;
        }
        Ok(updated)
    })
    .await
    .map_err(|e| e.to_string())??;
    if count > 0 {
        state.schedule_auto_backup();
        state.schedule_bibtex_auto_sync();
    }
    Ok(count)
}

#[tauri::command]
pub async fn batch_enqueue_ocr_cmd(
    state: State<'_, Arc<AppState>>,
    document_ids: Vec<i64>,
) -> Result<usize, String> {
    let db = Arc::clone(&state.db);
    let queued = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let w = db.writer();
        enqueue_ocr_for_documents(&w, &document_ids).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(queued)
}
