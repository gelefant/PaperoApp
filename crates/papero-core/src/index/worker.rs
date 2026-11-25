use crate::config::AppConfig;
use crate::db::repos::docs::{
    enqueue_index_job, mark_file_error, replace_file_pages_and_passages, FilePageInsert,
    PassageInsert,
};
use crate::db::Database;
use crate::dedup::{compute_document_minhash_from_pages, pack_minhash, run_duplicate_detection};
use crate::error::{CoreError, Result};
use crate::extract::chunker::chunk_document;
use crate::extract::djvu::extract_djvu;
use crate::extract::pdf::extract_pdf;
use crate::fs::roots::resolve_rel_path;
use crate::fs::scanner::ScanControl;
use crate::index::bm25::{Bm25Index, Bm25Passage};
use crate::index::vector::VectorIndex;
use crate::metadata::extract::extract_identifiers_and_heuristics_with_front_pages;
use crate::metadata::resolver::apply_extracted_identifiers_to_doc;
use crate::ml::{profile_is_ready, LazyModelSlot, ModelProfile, OnnxEmbedder};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thread_priority::{set_current_thread_priority, ThreadPriority};

#[derive(Debug, Clone)]
struct QueuedExtractJob {
    job_id: i64,
    file_id: i64,
    document_id: i64,
    root_id: String,
    root_path: String,
    rel_path: String,
    format: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkerBatchReport {
    pub processed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub passages_indexed: usize,
}

/// Processes all currently queued `extract_bm25` jobs at low OS thread priority,
/// pausing or cancelling cleanly when requested via `ScanControl`.
pub fn run_extract_bm25_queue(
    db: &Database,
    bm25: &Bm25Index,
    config: &AppConfig,
    control: Option<&ScanControl>,
) -> Result<WorkerBatchReport> {
    let _ = set_current_thread_priority(ThreadPriority::Min);
    let default_ctrl = ScanControl::new();
    let ctrl = control.unwrap_or(&default_ctrl);

    let jobs = load_queued_extract_jobs(db)?;
    let mut report = WorkerBatchReport::default();
    if jobs.is_empty() {
        return Ok(report);
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(config.max_worker_threads.max(1))
        .stack_size(8 * 1024 * 1024)
        .start_handler(|_| {
            let _ = set_current_thread_priority(ThreadPriority::Min);
        })
        .build()
        .map_err(|e| CoreError::Extract(format!("failed to build worker pool: {e}")))?;

    struct PreparedExtractOutput {
        page_count: u32,
        has_text_layer: bool,
        outline_json: Option<String>,
        page_inserts: Vec<FilePageInsert>,
        passage_inserts: Vec<PassageInsert>,
        ids: crate::metadata::models::ExtractedIdentifiers,
        minhash_blob: Option<Vec<u8>>,
    }

    // Process in compact batches of 8 files and drop each `ExtractedDocument` (pages/spans/raw_text)
    // immediately inside the worker closure so 600-page books never accumulate 25x in RAM!
    for chunk in jobs.chunks(8) {
        ctrl.wait_if_paused()?;

        let results: Vec<_> = pool.install(|| {
            use rayon::prelude::*;
            chunk
                .par_iter()
                .map(|job| -> Result<PreparedExtractOutput> {
                    if ctrl.is_cancelled() {
                        return Err(CoreError::Cancelled);
                    }
                    let inner = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                        || -> Result<PreparedExtractOutput> {
                            let full_path =
                                resolve_rel_path(Path::new(&job.root_path), &job.rel_path)?;
                            let extracted = match job.format.as_str() {
                                "pdf" => extract_pdf(&full_path, config.pdfium_lib_path.as_deref())?,
                                "djvu" => extract_djvu(&full_path, config.djvused_bin.as_deref())?,
                                other => {
                                    return Err(CoreError::CorruptFile(format!(
                                        "unsupported format: {other}"
                                    )));
                                }
                            };
                            let built_chunks = chunk_document(
                                &extracted,
                                config.chunk_target_tokens,
                                config.chunk_overlap_tokens,
                            );

                            let first_pages: Vec<&str> = extracted
                                .pages
                                .iter()
                                .take(6)
                                .map(|p| p.raw_text.as_str())
                                .collect();
                            let last_page = extracted.pages.last().map(|p| p.raw_text.as_str());
                            let stem = Path::new(&job.rel_path)
                                .file_stem()
                                .and_then(|s| s.to_str());
                            let ids = extract_identifiers_and_heuristics_with_front_pages(
                                extracted.xmp_xml.as_deref(),
                                extracted.doc_title.as_deref(),
                                extracted.doc_author.as_deref(),
                                &first_pages,
                                last_page,
                                extracted.page_count,
                                stem,
                                extracted.pages.first(),
                                extracted.pages.get(1),
                            );

                            let minhash_blob = compute_document_minhash_from_pages(
                                extracted
                                    .pages
                                    .iter()
                                    .map(|p| (p.page_ord, p.raw_text.as_str())),
                                extracted.page_count,
                            )
                            .map(|s| pack_minhash(&s));

                            let page_inserts: Vec<FilePageInsert> = extracted
                                .pages
                                .iter()
                                .map(|p| FilePageInsert {
                                    file_id: job.file_id,
                                    page_ord: p.page_ord,
                                    printed_label: p.printed_label.clone(),
                                    width_pts: p.width_pts,
                                    height_pts: p.height_pts,
                                    text_source: "native".to_string(),
                                })
                                .collect();

                            let outline_json = if extracted.outline.is_empty() {
                                None
                            } else {
                                serde_json::to_string(&extracted.outline).ok()
                            };
                            let page_count = extracted.page_count;
                            let has_text_layer = extracted.has_text_layer;
                            // Explicitly drop `extracted` (all raw_text + line spans) before building PassageInserts
                            drop(extracted);

                            let passage_inserts: Vec<PassageInsert> = built_chunks
                                .into_iter()
                                .map(|c| PassageInsert {
                                    document_id: job.document_id,
                                    file_id: job.file_id,
                                    page_ord: c.page_ord,
                                    printed_label: c.printed_label,
                                    chunk_index: c.chunk_index,
                                    is_alt_formula: false,
                                    token_count: c.token_count,
                                    text_content: c.text_content,
                                    bboxes_blob: c.bboxes_blob,
                                })
                                .collect();

                            Ok(PreparedExtractOutput {
                                page_count,
                                has_text_layer,
                                outline_json,
                                page_inserts,
                                passage_inserts,
                                ids,
                                minhash_blob,
                            })
                        },
                    ));
                    match inner {
                        Ok(res) => res,
                        Err(payload) => {
                            let detail = payload
                                .downcast_ref::<&str>()
                                .copied()
                                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                                .unwrap_or("unknown extraction panic");
                            Err(CoreError::CorruptFile(format!(
                                "panic while indexing {}: {detail}",
                                job.rel_path
                            )))
                        }
                    }
                })
                .collect()
        });

        if ctrl.is_cancelled() {
            return Err(CoreError::Cancelled);
        }

        let mut bm25_batch: Vec<Bm25Passage> = Vec::new();
        {
            let w = db.writer();
            for (job, res) in chunk.iter().zip(results) {
                if matches!(res, Err(CoreError::Cancelled)) {
                    return Err(CoreError::Cancelled);
                }
                report.processed += 1;
                match res {
                    Ok(prep) => {
                        let inserted_ids = replace_file_pages_and_passages(
                            &w,
                            job.file_id,
                            prep.page_count,
                            prep.has_text_layer,
                            prep.outline_json.as_deref(),
                            &prep.page_inserts,
                            &prep.passage_inserts,
                        )?;

                        // Move `text_content` directly into `Bm25Passage` without cloning!
                        for (pid, p) in inserted_ids.into_iter().zip(prep.passage_inserts) {
                            bm25_batch.push(Bm25Passage {
                                passage_id: pid,
                                document_id: job.document_id as u64,
                                file_id: job.file_id as u64,
                                root_id: job.root_id.clone(),
                                text_content: p.text_content,
                            });
                        }

                        let _ = apply_extracted_identifiers_to_doc(&w, job.document_id, &prep.ids, None);

                        let _ = w.execute(
                            "UPDATE documents SET minhash_sig = ?1 WHERE id = ?2",
                            params![prep.minhash_blob, job.document_id],
                        );
                        let _ = enqueue_index_job(&w, job.file_id, "metadata", 20);
                        let embed_stage = match ModelProfile::from_str_lossy(&config.ml_profile) {
                            ModelProfile::Light => "embed_light",
                            ModelProfile::Quality => "embed_quality",
                        };
                        let _ = w.execute(
                            "UPDATE files SET embedded_light = 0, embedded_quality = 0 WHERE id = ?1",
                            params![job.file_id],
                        );
                        let _ = enqueue_index_job(&w, job.file_id, embed_stage, 30);

                        w.execute("DELETE FROM index_jobs WHERE id = ?1", params![job.job_id])?;
                        report.succeeded += 1;
                    }
                    Err(err) => {
                        let state = match &err {
                            CoreError::EncryptedFile(_) => "encrypted",
                            CoreError::CorruptFile(_) => "corrupt",
                            _ => "io_error",
                        };
                        let msg = err.to_string();
                        mark_file_error(&w, job.file_id, state, &msg)?;
                        w.execute(
                            "UPDATE index_jobs SET status = 'failed', attempts = attempts + 1, last_error = ?1 WHERE id = ?2",
                            params![msg, job.job_id],
                        )?;
                        report.failed += 1;
                    }
                }
            }
        }

        if !bm25_batch.is_empty() {
            report.passages_indexed += bm25_batch.len();
            bm25.add_passages(&bm25_batch)?;
            bm25.commit()?;
        }
    }

    if report.succeeded > 0 {
        let _ = run_duplicate_detection(db);
    }

    Ok(report)
}

fn load_queued_extract_jobs(db: &Database) -> Result<Vec<QueuedExtractJob>> {
    let r = db.reader();
    let mut stmt = r.prepare(
        "SELECT j.id, f.id, f.document_id, f.root_id, sr.root_path, f.rel_path, f.format
         FROM index_jobs j
         JOIN files f ON f.id = j.file_id
         JOIN source_roots sr ON sr.id = f.root_id
         WHERE j.stage = 'extract_bm25' AND j.status = 'queued' AND sr.is_reachable = 1
         ORDER BY j.priority ASC, j.id ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(QueuedExtractJob {
            job_id: row.get(0)?,
            file_id: row.get(1)?,
            document_id: row.get(2)?,
            root_id: row.get(3)?,
            root_path: row.get(4)?,
            rel_path: row.get(5)?,
            format: row.get(6)?,
        })
    })?;
    let mut jobs = Vec::new();
    for row in rows {
        jobs.push(row?);
    }
    Ok(jobs)
}

#[derive(Debug, Clone)]
struct QueuedEmbedJob {
    job_id: Option<i64>,
    file_id: i64,
}

/// Returns `(total_embeddable_files, embedded_files)` for the given `profile` across all
/// currently reachable files that have a text layer (matching the exact `run_embed_queue` scope).
pub fn count_embed_progress(db: &Database, profile: ModelProfile) -> Result<(i64, i64)> {
    let embedded_col = match profile {
        ModelProfile::Light => "embedded_light",
        ModelProfile::Quality => "embedded_quality",
    };
    let r = db.reader();
    let sql = format!(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN f.{embedded_col} != 0 THEN 1 ELSE 0 END), 0)
         FROM files f
         JOIN source_roots sr ON sr.id = f.root_id
         WHERE sr.is_reachable = 1
           AND f.has_text_layer = 1"
    );
    let pair = r.query_row(&sql, [], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
    Ok(pair)
}

/// Phase 2 background embedding worker: generates vector embeddings for passages using
/// the active ONNX profile (`Light` or `Quality`), stores them into `VectorIndex`, and
/// updates `files.embedded_* = 1` + `index_state = 'ready'`.
/// Returns cleanly with zero work done if the ONNX models for `profile` are not downloaded yet.
pub fn run_embed_queue(
    db: &Database,
    vector_index: &VectorIndex,
    embedder_slot: &LazyModelSlot<OnnxEmbedder>,
    models_dir: &Path,
    profile: ModelProfile,
    config: &AppConfig,
    control: Option<&ScanControl>,
) -> Result<WorkerBatchReport> {
    run_embed_queue_with_progress(
        db,
        vector_index,
        embedder_slot,
        models_dir,
        profile,
        config,
        control,
        None,
    )
}

/// Same as [`run_embed_queue`], invoking `on_progress` whenever completed embeddings are flushed
/// to disk or when the worker starts/finishes/pauses.
#[allow(clippy::too_many_arguments)]
pub fn run_embed_queue_with_progress(
    db: &Database,
    vector_index: &VectorIndex,
    embedder_slot: &LazyModelSlot<OnnxEmbedder>,
    models_dir: &Path,
    profile: ModelProfile,
    config: &AppConfig,
    control: Option<&ScanControl>,
    on_progress: Option<&(dyn Fn() + Sync)>,
) -> Result<WorkerBatchReport> {
    let _ = set_current_thread_priority(ThreadPriority::Min);
    let mut report = WorkerBatchReport::default();
    if !profile_is_ready(models_dir, profile) {
        return Ok(report);
    }

    let stage = match profile {
        ModelProfile::Light => "embed_light",
        ModelProfile::Quality => "embed_quality",
    };
    let embedded_col = match profile {
        ModelProfile::Light => "embedded_light",
        ModelProfile::Quality => "embedded_quality",
    };

    let jobs = load_pending_embed_jobs(db, stage, embedded_col)?;
    if jobs.is_empty() {
        if let Some(cb) = on_progress {
            cb();
        }
        return Ok(report);
    }
    if let Some(cb) = on_progress {
        cb();
    }

    let default_ctrl = ScanControl::new();
    let ctrl = control.unwrap_or(&default_ctrl);
    let batch_size = match profile {
        ModelProfile::Light => 32usize,
        ModelProfile::Quality => 16usize,
    };
    // Cap background ONNX intra-op threads so `--ml-worker` runs quietly on Efficiency / low-power cores.
    let intra_threads = config.max_worker_threads.div_ceil(2).clamp(1, 3);
    let ort_lib = config.onnxruntime_lib_path.as_deref();

    const FLUSH_PASSAGE_THRESHOLD: usize = 1_200;
    const FLUSH_FILE_THRESHOLD: usize = 10;
    let mut pending_completed: Vec<(i64, Option<i64>)> = Vec::new();
    let mut buffered_passages: usize = 0;

    for job in jobs {
        if ctrl.is_paused() || ctrl.is_cancelled() {
            flush_completed_embed_jobs(db, vector_index, embedded_col, &mut pending_completed)?;
            buffered_passages = 0;
            if let Some(cb) = on_progress {
                cb();
            }
        }
        if ctrl.wait_if_paused().is_err() || ctrl.is_cancelled() {
            break;
        }

        let mut passages = load_file_passages_for_embedding(db, job.file_id)?;
        // Length-sort passages across the whole file so transformer batches avoid padding waste.
        passages.sort_by_key(|(_, t)| t.len());

        let mut to_upsert: Vec<(u64, Vec<f32>)> = Vec::with_capacity(passages.len());
        let mut failed_err: Option<String> = None;
        let mut stopped = false;

        for chunk in passages.chunks(batch_size) {
            if ctrl.is_paused() || ctrl.is_cancelled() {
                flush_completed_embed_jobs(db, vector_index, embedded_col, &mut pending_completed)?;
                buffered_passages = 0;
                if let Some(cb) = on_progress {
                    cb();
                }
            }
            if ctrl.wait_if_paused().is_err() || ctrl.is_cancelled() {
                stopped = true;
                break;
            }
            let texts: Vec<&str> = chunk.iter().map(|(_, t)| t.as_str()).collect();
            let emb_res = embedder_slot.with_model(
                || OnnxEmbedder::load_isolated(models_dir, profile, ort_lib, intra_threads),
                |emb| emb.embed_passages(&texts),
            );
            match emb_res {
                Ok(vecs) => {
                    for ((pid, _), vec) in chunk.iter().zip(vecs) {
                        to_upsert.push((*pid, vec));
                    }
                }
                Err(e) => {
                    failed_err = Some(e.to_string());
                    break;
                }
            }
        }

        if stopped {
            break;
        }

        report.processed += 1;

        if let Some(err) = failed_err {
            report.failed += 1;
            if let Some(jid) = job.job_id {
                let w = db.writer();
                let _ = w.execute(
                    "UPDATE index_jobs SET status = 'failed', attempts = attempts + 1, last_error = ?1 WHERE id = ?2",
                    params![err, jid],
                );
            }
            continue;
        }

        if !to_upsert.is_empty() {
            vector_index.upsert_in_memory(&to_upsert)?;
            report.passages_indexed += to_upsert.len();
            buffered_passages += to_upsert.len();
        }

        pending_completed.push((job.file_id, job.job_id));
        report.succeeded += 1;

        if buffered_passages >= FLUSH_PASSAGE_THRESHOLD
            || pending_completed.len() >= FLUSH_FILE_THRESHOLD
        {
            flush_completed_embed_jobs(db, vector_index, embedded_col, &mut pending_completed)?;
            buffered_passages = 0;
            if let Some(cb) = on_progress {
                cb();
            }
        }
    }

    flush_completed_embed_jobs(db, vector_index, embedded_col, &mut pending_completed)?;
    if let Some(cb) = on_progress {
        cb();
    }

    Ok(report)
}

fn flush_completed_embed_jobs(
    db: &Database,
    vector_index: &VectorIndex,
    embedded_col: &str,
    pending_completed: &mut Vec<(i64, Option<i64>)>,
) -> Result<()> {
    vector_index.flush()?;
    if pending_completed.is_empty() {
        return Ok(());
    }
    let now = chrono::Utc::now().timestamp();
    let update_sql = format!(
        "UPDATE files SET {embedded_col} = 1, index_state = 'ready', updated_at = ?1 WHERE id = ?2"
    );
    {
        let w = db.writer();
        let tx = w.unchecked_transaction()?;
        for (file_id, job_id) in pending_completed.iter() {
            tx.execute(&update_sql, params![now, file_id])?;
            if let Some(jid) = job_id {
                let _ = tx.execute("DELETE FROM index_jobs WHERE id = ?1", params![jid]);
            }
        }
        tx.commit()?;
    }
    pending_completed.clear();
    Ok(())
}

fn load_pending_embed_jobs(
    db: &Database,
    stage: &str,
    embedded_col: &str,
) -> Result<Vec<QueuedEmbedJob>> {
    let r = db.reader();
    let sql = format!(
        "SELECT f.id,
                (SELECT j.id FROM index_jobs j
                 WHERE j.file_id = f.id AND j.stage = ?1 AND j.status = 'queued'
                 ORDER BY j.id ASC LIMIT 1) AS job_id
         FROM files f
         JOIN source_roots sr ON sr.id = f.root_id
         WHERE sr.is_reachable = 1
           AND f.has_text_layer = 1
           AND f.{embedded_col} = 0
         ORDER BY f.id ASC"
    );
    let mut stmt = r.prepare(&sql)?;
    let rows = stmt.query_map(params![stage], |row| {
        Ok(QueuedEmbedJob {
            file_id: row.get(0)?,
            job_id: row.get(1)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn load_file_passages_for_embedding(db: &Database, file_id: i64) -> Result<Vec<(u64, String)>> {
    let r = db.reader();
    let mut stmt = r.prepare(
        "SELECT id, text_content FROM passages WHERE file_id = ?1 ORDER BY page_ord ASC, chunk_index ASC",
    )?;
    let rows = stmt.query_map(params![file_id], |row| {
        Ok((row.get::<_, i64>(0)? as u64, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}
