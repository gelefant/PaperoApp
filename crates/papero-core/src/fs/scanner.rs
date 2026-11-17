use crate::db::repos::archive::{apply_restored_archive, archive_if_manual, load_archive};
use crate::db::repos::docs::{
    count_files_for_document, create_heuristic_document, enqueue_index_job, get_file_passage_ids,
    insert_file_record, load_all_known_hashes, load_known_files_for_roots,
    record_level1_identical_copy, update_file_location, KnownFileRecord, NewFileRecord,
};
use crate::db::repos::roots::{
    count_files_in_root, list_source_roots, mark_root_scanned,
    prune_orphaned_taxonomy_and_duplicates, set_root_reachability, SourceRoot,
};
use crate::db::repos::scans::{create_scan_run, finish_scan_run, insert_scan_items, ScanRunItem, ScanSummary};
use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::fs::availability::{check_root_reachability, Reachability};
use crate::fs::roots::{detect_doc_format, to_posix_rel_path};
use crate::index::bm25::Bm25Index;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

pub const STATE_RUNNING: u8 = 0;
pub const STATE_PAUSED: u8 = 1;
pub const STATE_CANCELLED: u8 = 2;

/// Shared pause/cancel controller for background scans and indexing.
#[derive(Debug, Default)]
pub struct ScanControl {
    state: AtomicU8,
}

impl ScanControl {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(STATE_RUNNING),
        }
    }

    pub fn pause(&self) {
        let _ = self.state.compare_exchange(
            STATE_RUNNING,
            STATE_PAUSED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }

    pub fn resume(&self) {
        let _ = self.state.compare_exchange(
            STATE_PAUSED,
            STATE_RUNNING,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }

    pub fn cancel(&self) {
        self.state.store(STATE_CANCELLED, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.load(Ordering::SeqCst) == STATE_CANCELLED
    }

    pub fn wait_if_paused(&self) -> Result<()> {
        loop {
            match self.state.load(Ordering::SeqCst) {
                STATE_RUNNING => return Ok(()),
                STATE_CANCELLED => return Err(CoreError::Cancelled),
                _ => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }
    }
}

#[derive(Debug, Clone)]
struct DiskEntry {
    root_id: String,
    rel_path: String,
    full_path: PathBuf,
    format: &'static str,
    size_bytes: u64,
    mtime_ns: i64,
}

#[derive(Debug)]
struct HashedEntry {
    entry: DiskEntry,
    blake3_hash: std::result::Result<String, String>,
}

/// Runs an incremental rescan across all enabled source roots (or a single `only_root_id`).
pub fn rescan_sources(
    db: &Database,
    bm25: Option<&Bm25Index>,
    only_root_id: Option<&str>,
    control: Option<&ScanControl>,
) -> Result<ScanSummary> {
    let default_ctrl = ScanControl::new();
    let ctrl = control.unwrap_or(&default_ctrl);

    let all_roots = {
        let r = db.reader();
        list_source_roots(&r)?
    };

    let target_roots: Vec<SourceRoot> = all_roots
        .into_iter()
        .filter(|root| root.is_enabled && only_root_id.is_none_or(|id| id == root.id))
        .collect();

    let scan_id = {
        let w = db.writer();
        create_scan_run(&w)?
    };

    let mut summary = ScanSummary {
        scan_id,
        status: "running".to_string(),
        ..Default::default()
    };
    let mut items: Vec<ScanRunItem> = Vec::new();

    // Step 1: Check reachability BEFORE scanning or deleting anything.
    let mut reachable_roots = Vec::new();
    {
        let w = db.writer();
        for root in &target_roots {
            let prev_count = count_files_in_root(&w, &root.id)?;
            match check_root_reachability(Path::new(&root.root_path), prev_count) {
                Reachability::Reachable => {
                    set_root_reachability(&w, &root.id, true)?;
                    reachable_roots.push(root.clone());
                }
                Reachability::Unreachable { reason } => {
                    tracing::warn!(root_id = %root.id, reason = %reason, "source root unreachable; preserving catalog");
                    set_root_reachability(&w, &root.id, false)?;
                }
            }
        }
    }

    if reachable_roots.is_empty() {
        summary.status = "completed".to_string();
        let w = db.writer();
        finish_scan_run(&w, &summary)?;
        return Ok(summary);
    }

    let reachable_ids: Vec<String> = reachable_roots.iter().map(|r| r.id.clone()).collect();

    // Step 2: Preload known state in one query.
    let (known_for_reachable, all_known_files) = {
        let r = db.reader();
        (
            load_known_files_for_roots(&r, &reachable_ids)?,
            load_all_known_hashes(&r)?,
        )
    };

    let mut known_by_path: HashMap<(String, String), KnownFileRecord> =
        HashMap::with_capacity(known_for_reachable.len());
    for rec in known_for_reachable {
        known_by_path.insert((rec.root_id.clone(), rec.rel_path.clone()), rec);
    }

    let mut known_by_hash: HashMap<String, Vec<KnownFileRecord>> = HashMap::new();
    for rec in all_known_files {
        known_by_hash
            .entry(rec.blake3_hash.clone())
            .or_default()
            .push(rec);
    }

    // Step 3: Fast directory walk + stat comparison (never opens unchanged files!).
    let mut seen_paths: HashSet<(String, String)> = HashSet::with_capacity(known_by_path.len());
    let mut to_hash: Vec<DiskEntry> = Vec::new();

    for root in &reachable_roots {
        ctrl.wait_if_paused()?;
        let root_path = PathBuf::from(&root.root_path);

        for dent in WalkDir::new(&root_path).follow_links(false) {
            if ctrl.is_cancelled() {
                summary.status = "cancelled".to_string();
                let w = db.writer();
                finish_scan_run(&w, &summary)?;
                return Ok(summary);
            }
            let dent = match dent {
                Ok(d) => d,
                Err(e) => {
                    summary.cnt_errors += 1;
                    items.push(ScanRunItem {
                        category: "error".into(),
                        root_id: Some(root.id.clone()),
                        rel_path: e.path().map(|p| p.display().to_string()).unwrap_or_default(),
                        detail: Some(e.to_string()),
                    });
                    continue;
                }
            };
            if !dent.file_type().is_file() {
                continue;
            }
            let Some(format) = detect_doc_format(dent.path()) else {
                continue;
            };
            let rel_path = match to_posix_rel_path(&root_path, dent.path()) {
                Ok(p) => p,
                Err(e) => {
                    summary.cnt_errors += 1;
                    items.push(ScanRunItem {
                        category: "error".into(),
                        root_id: Some(root.id.clone()),
                        rel_path: dent.path().display().to_string(),
                        detail: Some(e.to_string()),
                    });
                    continue;
                }
            };

            let meta = match dent.metadata() {
                Ok(m) => m,
                Err(e) => {
                    summary.cnt_errors += 1;
                    items.push(ScanRunItem {
                        category: "error".into(),
                        root_id: Some(root.id.clone()),
                        rel_path,
                        detail: Some(e.to_string()),
                    });
                    continue;
                }
            };

            let size_bytes = meta.len();
            let mtime_ns = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64)
                .unwrap_or(0);

            let key = (root.id.clone(), rel_path.clone());
            seen_paths.insert(key.clone());

            if let Some(known) = known_by_path.get(&key) {
                if known.size_bytes == size_bytes && known.mtime_ns == mtime_ns {
                    summary.cnt_unchanged += 1;
                    continue;
                }
            }

            to_hash.push(DiskEntry {
                root_id: root.id.clone(),
                rel_path,
                full_path: dent.into_path(),
                format,
                size_bytes,
                mtime_ns,
            });
        }
    }

    // Step 4: Compute BLAKE3 only for new or stat-changed files (parallelized).
    let hashed: Vec<HashedEntry> = to_hash
        .into_par_iter()
        .map(|entry| {
            let res = compute_blake3_hex(&entry.full_path).map_err(|e| e.to_string());
            HashedEntry {
                entry,
                blake3_hash: res,
            }
        })
        .collect();

    // Step 5: Compute missing paths on REACHABLE roots only.
    let mut missing_by_path: HashMap<(String, String), KnownFileRecord> = HashMap::new();
    let mut missing_by_hash: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (key, rec) in &known_by_path {
        if !seen_paths.contains(key) {
            missing_by_path.insert(key.clone(), rec.clone());
            missing_by_hash
                .entry(rec.blake3_hash.clone())
                .or_default()
                .push(key.clone());
        }
    }

    // Step 6: Classify & apply changes in a single SQLite writer transaction.
    let mut tantivy_delete_passages: Vec<u64> = Vec::new();
    let mut tantivy_delete_docs: Vec<u64> = Vec::new();

    {
        let w = db.writer();
        let tx = w.unchecked_transaction()?;

        for item in hashed {
            let hash = match item.blake3_hash {
                Ok(h) => h,
                Err(err_msg) => {
                    summary.cnt_errors += 1;
                    items.push(ScanRunItem {
                        category: "error".into(),
                        root_id: Some(item.entry.root_id),
                        rel_path: item.entry.rel_path,
                        detail: Some(err_msg),
                    });
                    continue;
                }
            };

            let path_key = (item.entry.root_id.clone(), item.entry.rel_path.clone());

            // Case A: Path already existed -> check BLAKE3 before treating mtime change as modified!
            // Cross-OS cloud sync (e.g. macOS APFS <-> Windows NTFS Google Drive) frequently shifts
            // `mtime_ns` without altering bytes; avoid wiping extracted passages/embeddings in that case.
            if let Some(existing) = known_by_path.get(&path_key) {
                if existing.size_bytes == item.entry.size_bytes && existing.blake3_hash == hash {
                    tx.execute(
                        "UPDATE files SET mtime_ns = ?1, is_available = 1 WHERE id = ?2",
                        rusqlite::params![item.entry.mtime_ns, existing.file_id],
                    )?;
                    summary.cnt_unchanged += 1;
                    continue;
                }
                summary.cnt_modified += 1;
                let pids = get_file_passage_ids(&tx, existing.file_id)?;
                tantivy_delete_passages.extend(pids);

                tx.execute(
                    "DELETE FROM file_pages WHERE file_id = ?1",
                    rusqlite::params![existing.file_id],
                )?;
                tx.execute(
                    "DELETE FROM passages WHERE file_id = ?1",
                    rusqlite::params![existing.file_id],
                )?;
                let now = chrono::Utc::now().timestamp();
                tx.execute(
                    "UPDATE files SET size_bytes = ?1, mtime_ns = ?2, blake3_hash = ?3,
                            index_state = 'pending_extract', embedded_light = 0,
                            embedded_quality = 0, updated_at = ?4
                     WHERE id = ?5",
                    rusqlite::params![
                        item.entry.size_bytes as i64,
                        item.entry.mtime_ns,
                        hash,
                        now,
                        existing.file_id
                    ],
                )?;
                tx.execute(
                    "UPDATE documents SET content_modified_notice = 1, updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![now, existing.document_id],
                )?;
                enqueue_index_job(&tx, existing.file_id, "extract_bm25", 10)?;

                items.push(ScanRunItem {
                    category: "modified".into(),
                    root_id: Some(item.entry.root_id),
                    rel_path: item.entry.rel_path,
                    detail: Some(format!("document_id={}", existing.document_id)),
                });
                continue;
            }

            // Case B: Hash matches a missing file -> MOVED / RENAMED (zero reindexing!).
            let moved_from = missing_by_hash
                .get_mut(&hash)
                .and_then(|vec| vec.pop())
                .and_then(|old_key| missing_by_path.remove(&old_key));

            if let Some(old_rec) = moved_from {
                summary.cnt_moved_renamed += 1;
                update_file_location(
                    &tx,
                    old_rec.file_id,
                    &item.entry.root_id,
                    &item.entry.rel_path,
                    item.entry.size_bytes,
                    item.entry.mtime_ns,
                )?;
                items.push(ScanRunItem {
                    category: "moved".into(),
                    root_id: Some(item.entry.root_id),
                    rel_path: item.entry.rel_path,
                    detail: Some(format!("from {}", old_rec.rel_path)),
                });
                continue;
            }

            // Case C: Hash matches an existing file still present -> IDENTICAL COPY (Level 1 duplicate).
            if let Some(existing_docs) = known_by_hash.get(&hash) {
                if let Some(first) = existing_docs.first() {
                    summary.cnt_identical_copy += 1;
                    let new_file_id = insert_file_record(
                        &tx,
                        &NewFileRecord {
                            document_id: first.document_id,
                            root_id: &item.entry.root_id,
                            rel_path: &item.entry.rel_path,
                            format: item.entry.format,
                            size_bytes: item.entry.size_bytes,
                            mtime_ns: item.entry.mtime_ns,
                            blake3_hash: &hash,
                            index_state: "ready",
                        },
                    )?;
                    let is_new_group =
                        record_level1_identical_copy(&tx, first.document_id, &hash, new_file_id)?;
                    if is_new_group {
                        summary.cnt_dup_groups_new += 1;
                    }
                    items.push(ScanRunItem {
                        category: "copy".into(),
                        root_id: Some(item.entry.root_id),
                        rel_path: item.entry.rel_path,
                        detail: Some(format!("document_id={}", first.document_id)),
                    });
                    continue;
                }
            }

            // Case D: Brand NEW file (or reappeared from manual_metadata_archive!).
            summary.cnt_new += 1;
            let heuristic_title = heuristic_title_from_filename(&item.entry.full_path);
            let doc_id = create_heuristic_document(&tx, &heuristic_title)?;

            let mut restored_note = None;
            if let Some(archived) = load_archive(&tx, &hash)? {
                apply_restored_archive(&tx, doc_id, &archived)?;
                restored_note = Some("restored manual metadata & tags".to_string());
            }

            let file_id = insert_file_record(
                &tx,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: &item.entry.root_id,
                    rel_path: &item.entry.rel_path,
                    format: item.entry.format,
                    size_bytes: item.entry.size_bytes,
                    mtime_ns: item.entry.mtime_ns,
                    blake3_hash: &hash,
                    index_state: "pending_extract",
                },
            )?;
            enqueue_index_job(&tx, file_id, "extract_bm25", 10)?;

            // Track this new hash in case an identical copy appears in the same scan batch.
            known_by_hash.entry(hash.clone()).or_default().push(KnownFileRecord {
                file_id,
                document_id: doc_id,
                root_id: item.entry.root_id.clone(),
                rel_path: item.entry.rel_path.clone(),
                format: item.entry.format.to_string(),
                size_bytes: item.entry.size_bytes,
                mtime_ns: item.entry.mtime_ns,
                blake3_hash: hash,
            });

            items.push(ScanRunItem {
                category: "new".into(),
                root_id: Some(item.entry.root_id),
                rel_path: item.entry.rel_path,
                detail: restored_note,
            });
        }

        // Case E: Remaining missing files on reachable roots -> REMOVED.
        for ((_root_id, _rel), missing) in missing_by_path {
            summary.cnt_removed += 1;
            let pids = get_file_passage_ids(&tx, missing.file_id)?;
            tantivy_delete_passages.extend(pids);

            let remaining_before = count_files_for_document(&tx, missing.document_id)?;
            if remaining_before <= 1 {
                let _ = archive_if_manual(&tx, missing.document_id, &missing.blake3_hash)?;
                tx.execute(
                    "DELETE FROM documents WHERE id = ?1",
                    rusqlite::params![missing.document_id],
                )?;
                tantivy_delete_docs.push(missing.document_id as u64);
            } else {
                tx.execute(
                    "DELETE FROM files WHERE id = ?1",
                    rusqlite::params![missing.file_id],
                )?;
            }

            items.push(ScanRunItem {
                category: "removed".into(),
                root_id: Some(missing.root_id),
                rel_path: missing.rel_path,
                detail: Some(format!("blake3={}", missing.blake3_hash)),
            });
        }

        prune_orphaned_taxonomy_and_duplicates(&tx)?;

        let now = chrono::Utc::now().timestamp();
        for root in &reachable_roots {
            mark_root_scanned(&tx, &root.id, now)?;
        }

        insert_scan_items(&tx, scan_id, &items)?;
        summary.status = "completed".to_string();
        finish_scan_run(&tx, &summary)?;
        tx.commit()?;
    }

    // Apply deletions to Tantivy if an index handle was provided.
    if let Some(bm25_idx) = bm25 {
        if !tantivy_delete_passages.is_empty() || !tantivy_delete_docs.is_empty() {
            bm25_idx.delete_passages(&tantivy_delete_passages)?;
            for doc_id in tantivy_delete_docs {
                bm25_idx.delete_document(doc_id)?;
            }
            bm25_idx.commit()?;
        }
    }

    Ok(summary)
}

pub fn compute_blake3_hex(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn heuristic_title_from_filename(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.replace(['_', '-'], " ").trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Untitled Document".to_string())
}

/// Helper to spawn a background scan on a dedicated OS thread if desired.
pub fn spawn_rescan_thread(
    db: Arc<Database>,
    bm25: Option<Arc<Bm25Index>>,
    only_root_id: Option<String>,
    control: Arc<ScanControl>,
) -> std::thread::JoinHandle<Result<ScanSummary>> {
    std::thread::spawn(move || {
        rescan_sources(&db, bm25.as_deref(), only_root_id.as_deref(), Some(&control))
    })
}
