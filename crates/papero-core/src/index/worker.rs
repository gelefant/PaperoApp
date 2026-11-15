use crate::config::AppConfig;
use crate::db::repos::docs::{
    enqueue_index_job, mark_file_error, replace_file_pages_and_passages, FilePageInsert,
    PassageInsert,
};
use crate::db::Database;
use crate::dedup::{compute_minhash, pack_minhash, run_duplicate_detection};
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
        .start_handler(|_| {
            let _ = set_current_thread_priority(ThreadPriority::Min);
        })
        .build()
        .map_err(|e| CoreError::Extract(format!("failed to build worker pool: {e}")))?;

    for chunk in jobs.chunks(25) {
        ctrl.wait_if_paused()?;

        let results: Vec<_> = pool.install(|| {
            use rayon::prelude::*;
            chunk
                .par_iter()
                .map(|job| {
                    let full_path = resolve_rel_path(Path::new(&job.root_path), &job.rel_path)?;
                    let doc = match job.format.as_str() {
                        "pdf" => extract_pdf(&full_path, config.pdfium_lib_path.as_deref())?,
                        "djvu" => extract_djvu(&full_path, config.djvused_bin.as_deref())?,
                        other => {
                            return Err(CoreError::CorruptFile(format!(
                                "unsupported format: {other}"
                            )));
                        }
                    };
                    let chunks = chunk_document(
                        &doc,
                        config.chunk_target_tokens,
                        config.chunk_overlap_tokens,
                    );
                    Ok((doc, chunks))
                })
                .collect()
        });

        let mut bm25_batch: Vec<Bm25Passage> = Vec::new();
        {
            let w = db.writer();
            for (job, res) in chunk.iter().zip(results) {
                report.processed += 1;
                match res {
                    Ok((extracted, built_chunks)) => {
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

                        let passage_inserts: Vec<PassageInsert> = built_chunks
                            .iter()
                            .map(|c| PassageInsert {
                                document_id: job.document_id,
                                file_id: job.file_id,
                                page_ord: c.page_ord,
                                printed_label: c.printed_label.clone(),
                                chunk_index: c.chunk_index,
                                is_alt_formula: false,
                                token_count: c.token_count,
                                text_content: c.text_content.clone(),
                                bboxes_blob: c.bboxes_blob.clone(),
                            })
                            .collect();

                        let outline_json = if extracted.outline.is_empty() {
                            None
                        } else {
                            serde_json::to_string(&extracted.outline).ok()
                        };

                        let inserted_ids = replace_file_pages_and_passages(
                            &w,
                            job.file_id,
                            extracted.page_count,
                            extracted.has_text_layer,
                            outline_json.as_deref(),
                            &page_inserts,
                            &passage_inserts,
                        )?;

                        for (pid, p) in inserted_ids.into_iter().zip(passage_inserts) {
                            bm25_batch.push(Bm25Passage {
                                passage_id: pid,
                                document_id: job.document_id as u64,
                                file_id: job.file_id as u64,
                                root_id: job.root_id.clone(),
                                text_content: p.text_content,
                            });
                        }

                        // Extract local bibliographic identifiers, heuristics & MinHash signature
                        let first_pages: Vec<&str> = extracted
                            .pages
                            .iter()
                            .take(4)
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
                        let _ = apply_extracted_identifiers_to_doc(&w, job.document_id, &ids, None);

                        let full_text_for_minhash = extracted
                            .pages
                            .iter()
                            .take(15)
                            .map(|p| p.raw_text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n");
                        if let Some(sig) = compute_minhash(&full_text_for_minhash) {
                            let blob = pack_minhash(&sig);
                            let _ = w.execute(
                                "UPDATE documents SET minhash_sig = ?1 WHERE id = ?2",
                                params![blob, job.document_id],
                            );
                        }
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
        return Ok(report);
    }

    let default_ctrl = ScanControl::new();
    let ctrl = control.unwrap_or(&default_ctrl);
    let batch_size = match profile {
        ModelProfile::Light => 16usize,
        ModelProfile::Quality => 8usize,
    };
    let intra_threads = config.max_worker_threads.max(1);
    let ort_lib = config.onnxruntime_lib_path.as_deref();

    for job in jobs {
        if ctrl.wait_if_paused().is_err() || ctrl.is_cancelled() {
            break;
        }
        report.processed += 1;

        let passages = load_file_passages_for_embedding(db, job.file_id)?;
        let mut to_upsert: Vec<(u64, Vec<f32>)> = Vec::with_capacity(passages.len());
        let mut failed_err: Option<String> = None;

        for chunk in passages.chunks(batch_size) {
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
            vector_index.upsert_batch(&to_upsert)?;
            report.passages_indexed += to_upsert.len();
        }

        let now = chrono::Utc::now().timestamp();
        let w = db.writer();
        let update_sql = format!(
            "UPDATE files SET {embedded_col} = 1, index_state = 'ready', updated_at = ?1 WHERE id = ?2"
        );
        w.execute(&update_sql, params![now, job.file_id])?;
        if let Some(jid) = job.job_id {
            let _ = w.execute("DELETE FROM index_jobs WHERE id = ?1", params![jid]);
        }
        report.succeeded += 1;
    }

    Ok(report)
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
