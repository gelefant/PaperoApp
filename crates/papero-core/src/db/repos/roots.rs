use crate::db::repos::archive::archive_if_manual;
use crate::error::{CoreError, Result};
use crate::fs::availability::{check_root_reachability, Reachability};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct RemovedRootReport {
    pub deleted_file_ids: Vec<i64>,
    pub deleted_document_ids: Vec<i64>,
    pub deleted_passage_ids: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRoot {
    pub id: String,
    pub label: String,
    pub root_path: String,
    pub is_enabled: bool,
    pub is_network_or_cloud: bool,
    pub is_reachable: bool,
    pub auto_rescan_on_startup: bool,
    pub auto_rescan_interval_s: i64,
    pub exclude_from_cloud_llm: bool,
    pub last_scan_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewSourceRoot {
    pub id: String,
    pub label: String,
    pub root_path: String,
    pub is_network_or_cloud: bool,
}

pub fn insert_source_root(conn: &Connection, root: &NewSourceRoot) -> Result<SourceRoot> {
    let now = chrono::Utc::now().timestamp();
    let auto_startup: i64 = 0;
    conn.execute(
        "INSERT INTO source_roots (
            id, label, root_path, is_enabled, is_network_or_cloud, is_reachable,
            auto_rescan_on_startup, auto_rescan_interval_s, exclude_from_cloud_llm,
            last_scan_at, created_at
         ) VALUES (?1, ?2, ?3, 1, ?4, 1, ?5, 0, 0, NULL, ?6)",
        params![
            root.id,
            root.label,
            root.root_path,
            root.is_network_or_cloud as i64,
            auto_startup,
            now
        ],
    )?;
    get_source_root(conn, &root.id)?.ok_or_else(|| CoreError::RootNotFound(root.id.clone()))
}

pub fn get_source_root(conn: &Connection, id: &str) -> Result<Option<SourceRoot>> {
    conn.query_row(
        "SELECT id, label, root_path, is_enabled, is_network_or_cloud, is_reachable,
                auto_rescan_on_startup, auto_rescan_interval_s, exclude_from_cloud_llm,
                last_scan_at, created_at
         FROM source_roots WHERE id = ?1",
        params![id],
        row_to_source_root,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_source_roots(conn: &Connection) -> Result<Vec<SourceRoot>> {
    let mut stmt = conn.prepare(
        "SELECT id, label, root_path, is_enabled, is_network_or_cloud, is_reachable,
                auto_rescan_on_startup, auto_rescan_interval_s, exclude_from_cloud_llm,
                last_scan_at, created_at
         FROM source_roots ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map([], row_to_source_root)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn set_root_enabled(conn: &Connection, id: &str, enabled: bool) -> Result<()> {
    let updated = conn.execute(
        "UPDATE source_roots SET is_enabled = ?1 WHERE id = ?2",
        params![enabled as i64, id],
    )?;
    if updated == 0 {
        return Err(CoreError::RootNotFound(id.to_string()));
    }
    Ok(())
}

pub fn set_root_exclude_from_cloud_llm(conn: &Connection, id: &str, excluded: bool) -> Result<()> {
    let updated = conn.execute(
        "UPDATE source_roots SET exclude_from_cloud_llm = ?1 WHERE id = ?2",
        params![excluded as i64, id],
    )?;
    if updated == 0 {
        return Err(CoreError::RootNotFound(id.to_string()));
    }
    Ok(())
}

pub fn update_root_path(conn: &Connection, id: &str, new_root_path: &str) -> Result<()> {
    let updated = conn.execute(
        "UPDATE source_roots SET root_path = ?1 WHERE id = ?2",
        params![new_root_path, id],
    )?;
    if updated == 0 {
        return Err(CoreError::RootNotFound(id.to_string()));
    }
    Ok(())
}

/// Strips Windows `\\?\` / `\\?\UNC\` verbatim path prefixes so stored paths stay readable
/// and compatible with external tools (Explorer, SumatraPDF, DjView).
pub fn strip_windows_verbatim_prefix(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        if !rest.starts_with("Volume{") {
            return rest.to_string();
        }
    }
    raw.to_string()
}

/// Canonicalizes a directory path while keeping Windows drive-letter paths clean.
pub fn normalize_root_dir_path(path: &Path) -> String {
    match fs::canonicalize(path) {
        Ok(canon) => {
            let s = canon.to_string_lossy().into_owned();
            #[cfg(target_os = "windows")]
            {
                if s.starts_with(r"\\?\Volume{") && path.is_dir() {
                    path.to_string_lossy().into_owned()
                } else {
                    strip_windows_verbatim_prefix(&s)
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                s
            }
        }
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

pub fn load_local_root_paths(path: &Path) -> HashMap<String, String> {
    let Ok(raw) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_local_root_paths(path: &Path, map: &HashMap<String, String>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(map)?;
    fs::write(path, json)?;
    Ok(())
}

pub fn record_local_root_path(path: &Path, root_id: &str, root_path: &str) -> Result<()> {
    let mut map = load_local_root_paths(path);
    if map.get(root_id).map(String::as_str) == Some(root_path) {
        return Ok(());
    }
    map.insert(root_id.to_string(), root_path.to_string());
    save_local_root_paths(path, &map)
}

pub fn remove_local_root_path(path: &Path, root_id: &str) -> Result<()> {
    let mut map = load_local_root_paths(path);
    if map.remove(root_id).is_some() {
        save_local_root_paths(path, &map)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileRootsReport {
    pub remapped: usize,
    pub reachable: usize,
    pub unreachable: usize,
}

/// Reconciles `source_roots` against the local filesystem and `local_root_paths.json`.
/// When a backup created on another OS is restored, any `root_id` already rebound on this
/// device is automatically remapped; otherwise unreachable roots are flagged cleanly without
/// deleting any catalog data.
pub fn reconcile_source_roots_with_local_map(
    conn: &Connection,
    local_map_path: &Path,
) -> Result<ReconcileRootsReport> {
    let roots = list_source_roots(conn)?;
    let mut local_map = load_local_root_paths(local_map_path);
    let mut map_dirty = false;
    let mut report = ReconcileRootsReport::default();

    for root in roots {
        let prev_count = count_files_in_root(conn, &root.id)?;
        let current_ok =
            check_root_reachability(Path::new(&root.root_path), prev_count) == Reachability::Reachable;
        if current_ok {
            set_root_reachability(conn, &root.id, true)?;
            if local_map.get(&root.id).map(String::as_str) != Some(root.root_path.as_str()) {
                local_map.insert(root.id.clone(), root.root_path.clone());
                map_dirty = true;
            }
            report.reachable += 1;
            continue;
        }

        if let Some(mapped) = local_map.get(&root.id).cloned() {
            if mapped != root.root_path
                && check_root_reachability(Path::new(&mapped), prev_count) == Reachability::Reachable
            {
                update_root_path(conn, &root.id, &mapped)?;
                set_root_reachability(conn, &root.id, true)?;
                report.remapped += 1;
                report.reachable += 1;
                continue;
            }
        }

        set_root_reachability(conn, &root.id, false)?;
        report.unreachable += 1;
    }

    if map_dirty {
        save_local_root_paths(local_map_path, &local_map)?;
    }
    Ok(report)
}

/// Rebinds an existing source root to a new native directory path on this device, marks all
/// files in that root available, and remembers the mapping locally for future cross-OS restores.
pub fn rebind_source_root_path(
    conn: &Connection,
    local_map_path: &Path,
    root_id: &str,
    new_root_path: &str,
) -> Result<SourceRoot> {
    update_root_path(conn, root_id, new_root_path)?;
    set_root_reachability(conn, root_id, true)?;
    record_local_root_path(local_map_path, root_id, new_root_path)?;
    get_source_root(conn, root_id)?.ok_or_else(|| CoreError::RootNotFound(root_id.to_string()))
}

/// Cleans up orphaned tags, empty topic clusters, and `<2`-member duplicate groups.
pub fn prune_orphaned_taxonomy_and_duplicates(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM tags
         WHERE NOT EXISTS (SELECT 1 FROM document_tags dt WHERE dt.tag_id = tags.id)",
        [],
    )?;
    conn.execute(
        "DELETE FROM topic_clusters
         WHERE NOT EXISTS (SELECT 1 FROM document_clusters dc WHERE dc.cluster_id = topic_clusters.id)",
        [],
    )?;
    conn.execute(
        "DELETE FROM duplicate_groups
         WHERE id NOT IN (
             SELECT group_id FROM duplicate_group_members
             GROUP BY group_id
             HAVING COUNT(*) >= 2
         )",
        [],
    )?;
    conn.execute(
        "DELETE FROM duplicate_evaluated_pairs
         WHERE NOT EXISTS (SELECT 1 FROM documents d WHERE d.id = duplicate_evaluated_pairs.doc_id_min)
            OR NOT EXISTS (SELECT 1 FROM documents d WHERE d.id = duplicate_evaluated_pairs.doc_id_max)",
        [],
    )?;
    Ok(())
}

/// Deletes any `documents` that no longer have physical `files` rows (e.g. after removing a
/// source folder) along with cascading MSC2020 classifications, orphan tags, empty clusters,
/// and degenerate duplicate groups. Returns the list of deleted `document_id`s.
pub fn prune_orphaned_catalog_entities(conn: &Connection) -> Result<Vec<i64>> {
    let orphan_doc_ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM documents
             WHERE NOT EXISTS (SELECT 1 FROM files f WHERE f.document_id = documents.id)",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    if !orphan_doc_ids.is_empty() {
        conn.execute(
            "DELETE FROM documents
             WHERE NOT EXISTS (SELECT 1 FROM files f WHERE f.document_id = documents.id)",
            [],
        )?;
    }

    prune_orphaned_taxonomy_and_duplicates(conn)?;
    Ok(orphan_doc_ids)
}

/// Removes a source folder, archives manual metadata/MSC/tags of documents exclusively in that
/// folder, deletes orphaned documents (clearing MSC2020, tags, clusters, and duplicate groups),
/// and returns deleted file/document/passage IDs so search indices can be updated synchronously.
pub fn remove_source_root_with_cleanup(conn: &Connection, id: &str) -> Result<RemovedRootReport> {
    let tx = conn.unchecked_transaction()?;

    let exists: Option<i64> = tx
        .query_row(
            "SELECT 1 FROM source_roots WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?;
    if exists.is_none() {
        return Err(CoreError::RootNotFound(id.to_string()));
    }

    let root_files: Vec<(i64, i64, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, document_id, blake3_hash FROM files WHERE root_id = ?1",
        )?;
        let rows = stmt.query_map(params![id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let deleted_passage_ids: Vec<u64> = {
        let mut stmt = tx.prepare(
            "SELECT p.id
             FROM passages p
             JOIN files f ON f.id = p.file_id
             WHERE f.root_id = ?1",
        )?;
        let rows = stmt.query_map(params![id], |r| Ok(r.get::<_, i64>(0)? as u64))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let mut archived_docs = HashSet::new();
    for (_fid, doc_id, hash) in &root_files {
        if archived_docs.insert(*doc_id) {
            let other_files: i64 = tx.query_row(
                "SELECT COUNT(*) FROM files WHERE document_id = ?1 AND root_id != ?2",
                params![doc_id, id],
                |r| r.get(0),
            )?;
            if other_files == 0 {
                let _ = archive_if_manual(&tx, *doc_id, hash)?;
            }
        }
    }

    let deleted = tx.execute("DELETE FROM source_roots WHERE id = ?1", params![id])?;
    if deleted == 0 {
        return Err(CoreError::RootNotFound(id.to_string()));
    }

    let deleted_document_ids = prune_orphaned_catalog_entities(&tx)?;
    tx.commit()?;

    Ok(RemovedRootReport {
        deleted_file_ids: root_files.into_iter().map(|(fid, _, _)| fid).collect(),
        deleted_document_ids,
        deleted_passage_ids,
    })
}

pub fn remove_source_root(conn: &Connection, id: &str) -> Result<()> {
    let _ = remove_source_root_with_cleanup(conn, id)?;
    Ok(())
}

pub fn count_files_in_root(conn: &Connection, root_id: &str) -> Result<i64> {
    let cnt: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE root_id = ?1",
        params![root_id],
        |row| row.get(0),
    )?;
    Ok(cnt)
}

pub fn set_root_reachability(conn: &Connection, root_id: &str, reachable: bool) -> Result<()> {
    conn.execute(
        "UPDATE source_roots SET is_reachable = ?1 WHERE id = ?2",
        params![reachable as i64, root_id],
    )?;
    conn.execute(
        "UPDATE files SET is_available = ?1 WHERE root_id = ?2",
        params![reachable as i64, root_id],
    )?;
    Ok(())
}

pub fn mark_root_scanned(conn: &Connection, root_id: &str, timestamp: i64) -> Result<()> {
    conn.execute(
        "UPDATE source_roots SET last_scan_at = ?1 WHERE id = ?2",
        params![timestamp, root_id],
    )?;
    Ok(())
}

fn row_to_source_root(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceRoot> {
    Ok(SourceRoot {
        id: row.get(0)?,
        label: row.get(1)?,
        root_path: row.get(2)?,
        is_enabled: row.get::<_, i64>(3)? != 0,
        is_network_or_cloud: row.get::<_, i64>(4)? != 0,
        is_reachable: row.get::<_, i64>(5)? != 0,
        auto_rescan_on_startup: row.get::<_, i64>(6)? != 0,
        auto_rescan_interval_s: row.get(7)?,
        exclude_from_cloud_llm: row.get::<_, i64>(8)? != 0,
        last_scan_at: row.get(9)?,
        created_at: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::{add_tag_to_document, build_msc_tree, list_all_tags, list_topic_clusters};
    use crate::db::repos::archive::load_archive;
    use crate::db::repos::docs::{
        create_heuristic_document, insert_file_record, list_documents_for_ui, NewFileRecord,
    };
    use crate::db::Database;
    use tempfile::tempdir;

    #[test]
    fn removing_source_root_cleans_msc2020_tags_clusters_and_archives_manual_edits() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("roots_cleanup.db"), 2).unwrap();
        let w = db.writer();

        insert_source_root(
            &w,
            &NewSourceRoot {
                id: "root_num".into(),
                label: "Numerical Analysis".into(),
                root_path: "/tmp/num".into(),
                is_network_or_cloud: false,
            },
        )
        .unwrap();

        let doc_id = create_heuristic_document(&w, "Finite Elements on Polytopal Meshes").unwrap();
        insert_file_record(
            &w,
            &NewFileRecord {
                document_id: doc_id,
                root_id: "root_num",
                rel_path: "fem.pdf",
                format: "pdf",
                size_bytes: 1024,
                mtime_ns: 100,
                blake3_hash: "deadbeef1234567890",
                index_state: "ready",
            },
        )
        .unwrap();

        w.execute(
            "UPDATE documents
             SET msc_primary = '65N30', msc_primary_status = 'confirmed',
                 manual_locked_fields = '[\"msc_primary\"]'
             WHERE id = ?1",
            params![doc_id],
        )
        .unwrap();
        w.execute(
            "INSERT INTO document_msc_secondary (document_id, msc_code, status)
             VALUES (?1, '68W40', 'metadata')",
            params![doc_id],
        )
        .unwrap();
        add_tag_to_document(&w, doc_id, "fem").unwrap();
        w.execute(
            "INSERT INTO topic_clusters (model_profile, label, is_label_locked, keywords_json, updated_at)
             VALUES ('light', 'FEM Cluster', 0, '[\"fem\"]', 1)",
            [],
        )
        .unwrap();
        let cluster_id = w.last_insert_rowid();
        w.execute(
            "INSERT INTO document_clusters (document_id, cluster_id, is_user_assigned, score)
             VALUES (?1, ?2, 0, 1.0)",
            params![doc_id, cluster_id],
        )
        .unwrap();

        // Sanity check BEFORE folder removal: MSC 65 & 68, tag 'fem', and cluster are populated.
        let tree_before = build_msc_tree(&w).unwrap();
        assert_eq!(
            tree_before.iter().find(|n| n.code == "65").unwrap().total_count,
            1
        );
        assert_eq!(
            tree_before.iter().find(|n| n.code == "68").unwrap().total_count,
            1
        );
        assert_eq!(list_all_tags(&w).unwrap().len(), 1);
        assert_eq!(list_topic_clusters(&w).unwrap().len(), 1);

        // Remove the source folder!
        let report = remove_source_root_with_cleanup(&w, "root_num").unwrap();
        assert_eq!(report.deleted_document_ids, vec![doc_id]);
        assert_eq!(list_documents_for_ui(&w).unwrap().len(), 0);

        // MSC 2020 tree, tags, and clusters must all be completely cleared!
        let tree_after = build_msc_tree(&w).unwrap();
        for node in &tree_after {
            assert_eq!(
                node.total_count, 0,
                "expected MSC {} count to be 0 after removing folder, got {}",
                node.code, node.total_count
            );
            assert!(node.children.is_empty());
        }
        assert!(list_all_tags(&w).unwrap().is_empty());
        assert!(list_topic_clusters(&w).unwrap().is_empty());

        // Manual MSC edit & tag were preserved in manual_metadata_archive keyed by BLAKE3!
        let archived = load_archive(&w, "deadbeef1234567890").unwrap().unwrap();
        assert_eq!(archived.snapshot.msc_primary.as_deref(), Some("65N30"));
        assert_eq!(archived.tags, vec!["fem".to_string()]);
    }

    #[test]
    fn strip_windows_verbatim_prefix_normalizes_drive_and_unc_paths() {
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\G:\Il mio Drive\Papers"),
            r"G:\Il mio Drive\Papers"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\UNC\server\share\Papers"),
            r"\\server\share\Papers"
        );
        assert_eq!(
            strip_windows_verbatim_prefix(r"\\?\Volume{1234}\Papers"),
            r"\\?\Volume{1234}\Papers"
        );
    }
}
