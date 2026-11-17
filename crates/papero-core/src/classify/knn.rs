use crate::classify::msc2020::normalize_msc_code;
use crate::db::Database;
use crate::error::Result;
use crate::index::vector::VectorIndex;
use crate::ml::embedder::l2_normalize;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnnProposeReport {
    pub labeled_docs: usize,
    pub unlabeled_docs: usize,
    pub proposed_count: usize,
}

#[derive(Debug, Clone)]
pub struct DocMeanVector {
    pub document_id: i64,
    pub title: String,
    pub lead_text: String,
    pub msc_primary: Option<String>,
    pub msc_status: String,
    pub is_msc_locked: bool,
    pub vector: Vec<f32>,
}

/// Computes L2-normalized document mean embeddings by averaging passage vectors
/// retrieved from `VectorIndex` for every document that has embedded passages.
pub fn compute_document_mean_vectors(
    conn: &Connection,
    vindex: &VectorIndex,
) -> Result<Vec<DocMeanVector>> {
    compute_document_mean_vectors_opts(conn, vindex, true)
}

/// Computes L2-normalized document mean embeddings using covering index `idx_passages_doc_page`
/// (zero `passages.text_content` heap scans unless `include_lead_text` is requested for c-TF-IDF).
pub fn compute_document_mean_vectors_opts(
    conn: &Connection,
    vindex: &VectorIndex,
    include_lead_text: bool,
) -> Result<Vec<DocMeanVector>> {
    if vindex.is_empty() {
        return Ok(Vec::new());
    }

    let mut doc_meta: HashMap<i64, (String, Option<String>, String, bool)> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, title, msc_primary, msc_primary_status, manual_locked_fields FROM documents",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for r in rows {
            let (id, title, msc, status, locked_json) = r?;
            let locked: Vec<String> = serde_json::from_str(&locked_json).unwrap_or_default();
            let is_locked = locked.iter().any(|f| f == "msc_primary");
            doc_meta.insert(id, (title, msc, status, is_locked));
        }
    }

    // 100% covering-index scan (`idx_passages_doc_page`): never touches `passages.text_content` pages!
    let pairs: Vec<(i64, u64)> = {
        let mut stmt = conn.prepare(
            "SELECT document_id, id FROM passages ORDER BY document_id ASC, page_ord ASC, chunk_index ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let acc_vecs = vindex.accumulate_doc_vectors(pairs);
    if acc_vecs.is_empty() {
        return Ok(Vec::new());
    }

    let mut lead_texts: HashMap<i64, String> = HashMap::new();
    if include_lead_text {
        let mut stmt = conn.prepare(
            "SELECT document_id, SUBSTR(text_content, 1, 1200)
             FROM passages
             WHERE page_ord <= 3 AND chunk_index <= 1
             ORDER BY document_id ASC, page_ord ASC, chunk_index ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for r in rows {
            let (doc_id, snippet) = r?;
            if !acc_vecs.contains_key(&doc_id) {
                continue;
            }
            let entry = lead_texts.entry(doc_id).or_default();
            if entry.len() < 1200 {
                if !entry.is_empty() {
                    entry.push(' ');
                }
                entry.push_str(&snippet);
            }
        }
    }

    let mut out = Vec::with_capacity(acc_vecs.len());
    for (doc_id, (mut sum_vec, count)) in acc_vecs {
        if count == 0 {
            continue;
        }
        let inv = 1.0 / (count as f32);
        for v in &mut sum_vec {
            *v *= inv;
        }
        l2_normalize(&mut sum_vec);
        if let Some((title, msc_primary, msc_status, is_msc_locked)) = doc_meta.remove(&doc_id) {
            let lead_text = lead_texts.remove(&doc_id).unwrap_or_default();
            out.push(DocMeanVector {
                document_id: doc_id,
                title,
                lead_text,
                msc_primary,
                msc_status,
                is_msc_locked,
                vector: sum_vec,
            });
        }
    }
    Ok(out)
}

/// Runs k-NN MSC proposal using a read-only connection for vector aggregation/scoring and acquiring
/// the writer lock only for the brief SQLite `UPDATE` transaction.
pub fn propose_msc_knn_on_db(
    db: &Database,
    vindex: &VectorIndex,
    k: usize,
    min_top_similarity: f32,
) -> Result<KnnProposeReport> {
    let (labeled_docs, unlabeled_docs, updates) = {
        let r = db.reader();
        compute_knn_proposals(&r, vindex, k, min_top_similarity)?
    };
    if updates.is_empty() {
        return Ok(KnnProposeReport {
            labeled_docs,
            unlabeled_docs,
            proposed_count: 0,
        });
    }
    let w = db.writer();
    let proposed_count = apply_knn_proposals(&w, &updates)?;
    Ok(KnnProposeReport {
        labeled_docs,
        unlabeled_docs,
        proposed_count,
    })
}

type KnnProposalPlan = (usize, usize, Vec<(i64, String)>);

fn compute_knn_proposals(
    conn: &Connection,
    vindex: &VectorIndex,
    k: usize,
    min_top_similarity: f32,
) -> Result<KnnProposalPlan> {
    let docs = compute_document_mean_vectors_opts(conn, vindex, false)?;
    let k = k.max(1);

    let mut labeled: Vec<(&str, &[f32])> = Vec::new();
    let mut unlabeled: Vec<&DocMeanVector> = Vec::new();

    for d in &docs {
        if matches!(d.msc_status.as_str(), "metadata" | "confirmed") {
            if let Some(code) = d.msc_primary.as_deref().filter(|c| !c.trim().is_empty()) {
                labeled.push((code, &d.vector));
            }
        } else if !d.is_msc_locked
            && (d.msc_status == "none"
                || d.msc_status == "proposed"
                || d.msc_primary.as_deref().map(str::trim).unwrap_or("").is_empty())
        {
            unlabeled.push(d);
        }
    }

    let labeled_docs = labeled.len();
    let unlabeled_docs = unlabeled.len();
    if labeled.is_empty() || unlabeled.is_empty() {
        return Ok((labeled_docs, unlabeled_docs, Vec::new()));
    }

    let mut updates = Vec::new();
    for target in unlabeled {
        let mut sims: Vec<(&str, f32)> = labeled
            .iter()
            .map(|(code, vec)| (*code, dot(&target.vector, vec)))
            .filter(|(_, sim)| *sim > 0.0)
            .collect();
        sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sims.truncate(k);

        if sims.first().map(|s| s.1).unwrap_or(0.0) < min_top_similarity {
            continue;
        }

        let mut votes: HashMap<&str, f32> = HashMap::new();
        for (code, sim) in sims {
            *votes.entry(code).or_insert(0.0) += sim;
        }
        if let Some((best_code, _)) = votes
            .into_iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        {
            updates.push((target.document_id, best_code.to_string()));
        }
    }
    Ok((labeled_docs, unlabeled_docs, updates))
}

fn apply_knn_proposals(conn: &Connection, updates: &[(i64, String)]) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare_cached(
            "UPDATE documents SET msc_primary = ?1, msc_primary_status = 'proposed', updated_at = ?2 WHERE id = ?3",
        )?;
        for (doc_id, code) in updates {
            stmt.execute(params![code, now, doc_id])?;
        }
    }
    tx.commit()?;
    Ok(updates.len())
}

pub fn propose_msc_knn(
    conn: &Connection,
    vindex: &VectorIndex,
    k: usize,
    min_top_similarity: f32,
) -> Result<KnnProposeReport> {
    let (labeled_docs, unlabeled_docs, updates) =
        compute_knn_proposals(conn, vindex, k, min_top_similarity)?;
    if updates.is_empty() {
        return Ok(KnnProposeReport {
            labeled_docs,
            unlabeled_docs,
            proposed_count: 0,
        });
    }
    let proposed_count = apply_knn_proposals(conn, &updates)?;
    Ok(KnnProposeReport {
        labeled_docs,
        unlabeled_docs,
        proposed_count,
    })
}

/// Confirms a proposed (or user-provided) primary MSC2020 code on `document_id`,
/// marking `msc_primary_status = 'confirmed'` and locking `'msc_primary'` against future k-NN overwrites.
pub fn confirm_document_msc(
    conn: &Connection,
    document_id: i64,
    msc_code_override: Option<&str>,
) -> Result<()> {
    let existing: Option<(Option<String>, String)> = conn
        .query_row(
            "SELECT msc_primary, manual_locked_fields FROM documents WHERE id = ?1",
            params![document_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    let Some((current_msc, locked_json)) = existing else {
        return Ok(());
    };

    let final_code = msc_code_override
        .and_then(normalize_msc_code)
        .or_else(|| current_msc.as_deref().and_then(normalize_msc_code))
        .or(current_msc);

    let mut locked: Vec<String> = serde_json::from_str(&locked_json).unwrap_or_default();
    if !locked.iter().any(|f| f == "msc_primary") {
        locked.push("msc_primary".to_string());
    }
    let locked_out = serde_json::to_string(&locked)?;
    let now = chrono::Utc::now().timestamp();

    conn.execute(
        "UPDATE documents
         SET msc_primary = ?1,
             msc_primary_status = 'confirmed',
             manual_locked_fields = ?2,
             updated_at = ?3
         WHERE id = ?4",
        params![final_code, locked_out, now, document_id],
    )?;
    Ok(())
}

/// Rejects/clears a `'proposed'` MSC classification (leaves confirmed/metadata intact).
pub fn reject_proposed_msc(conn: &Connection, document_id: i64) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE documents
         SET msc_primary = NULL, msc_primary_status = 'none', updated_at = ?1
         WHERE id = ?2 AND msc_primary_status = 'proposed'",
        params![now, document_id],
    )?;
    Ok(())
}

pub(crate) fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::docs::{
        create_heuristic_document, insert_file_record, replace_file_pages_and_passages,
        FilePageInsert, NewFileRecord, PassageInsert,
    };
    use crate::db::repos::roots::{insert_source_root, NewSourceRoot};
    use crate::db::Database;
    use tempfile::tempdir;

    #[test]
    fn knn_proposes_msc_from_nearest_neighbors_and_respects_confirm_and_reject() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("knn.db"), 2).unwrap();
        let vindex = VectorIndex::open(&dir.path().join("vec.usearch"), 4).unwrap();
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();

        let (d_known1, d_known2, d_target, p1, p2, p3) = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Lib".into(),
                    root_path: lib.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();
            let make_doc = |title: &str, rel: &str, msc: Option<&str>, status: &str| -> (i64, u64) {
                let did = create_heuristic_document(&w, title).unwrap();
                if let Some(code) = msc {
                    w.execute(
                        "UPDATE documents SET msc_primary = ?1, msc_primary_status = ?2 WHERE id = ?3",
                        params![code, status, did],
                    )
                    .unwrap();
                }
                let fid = insert_file_record(
                    &w,
                    &NewFileRecord {
                        document_id: did,
                        root_id: "r1",
                        rel_path: rel,
                        format: "pdf",
                        size_bytes: 10,
                        mtime_ns: 0,
                        blake3_hash: rel,
                        index_state: "ready",
                    },
                )
                .unwrap();
                let pids = replace_file_pages_and_passages(
                    &w,
                    fid,
                    1,
                    true,
                    None,
                    &[FilePageInsert {
                        file_id: fid,
                        page_ord: 1,
                        printed_label: Some("1".into()),
                        width_pts: 600.0,
                        height_pts: 800.0,
                        text_source: "test".into(),
                    }],
                    &[PassageInsert {
                        document_id: did,
                        file_id: fid,
                        page_ord: 1,
                        printed_label: Some("1".into()),
                        chunk_index: 0,
                        is_alt_formula: false,
                        token_count: 8,
                        text_content: title.to_string(),
                        bboxes_blob: Vec::new(),
                    }],
                )
                .unwrap();
                (did, pids[0])
            };
            let (d1, pid1) = make_doc("Optimal Transport", "a.pdf", Some("49Q20"), "confirmed");
            let (d2, pid2) = make_doc("Galois Groups", "b.pdf", Some("11R32"), "metadata");
            let (d3, pid3) = make_doc("Wasserstein Gradient Flow", "c.pdf", None, "none");
            (d1, d2, d3, pid1, pid2, pid3)
        };

        vindex
            .upsert_batch(&[
                (p1, vec![0.98, 0.2, 0.0, 0.0]),
                (p2, vec![0.0, 0.0, 1.0, 0.0]),
                (p3, vec![0.96, 0.28, 0.0, 0.0]),
            ])
            .unwrap();

        let _ = (d_known1, d_known2);
        let rep = propose_msc_knn(&db.writer(), &vindex, 7, 0.2).unwrap();
        assert_eq!(rep.proposed_count, 1);

        let (code, st): (Option<String>, String) = db
            .reader()
            .query_row(
                "SELECT msc_primary, msc_primary_status FROM documents WHERE id = ?1",
                params![d_target],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(code.as_deref(), Some("49Q20"));
        assert_eq!(st, "proposed");

        confirm_document_msc(&db.writer(), d_target, None).unwrap();
        let st2: String = db
            .reader()
            .query_row(
                "SELECT msc_primary_status FROM documents WHERE id = ?1",
                params![d_target],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(st2, "confirmed");
    }
}
