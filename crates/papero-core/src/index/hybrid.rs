use crate::db::repos::docs::{ContentSearchDocResult, ContentSearchPassageHit};
use crate::error::Result;
use crate::extract::types::unpack_bboxes;
use crate::index::bm25::{Bm25Hit, Bm25Index};
use crate::index::vector::{VectorHit, VectorIndex};
use crate::ml::{LazyModelSlot, OnnxEmbedder, OnnxReranker};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub const DEFAULT_RRF_K: f32 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocScoreAggregation {
    Max,
    Top2Mean,
}

impl DocScoreAggregation {
    pub fn from_str_lossy(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "top2_mean" | "top2mean" => Self::Top2Mean,
            _ => Self::Max,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchOptions {
    pub top_n_per_branch: usize,
    pub rerank_enabled: bool,
    pub rerank_top_m: usize,
    pub top_k_docs: usize,
    pub min_score: f32,
    pub rrf_k: f32,
    pub aggregation: DocScoreAggregation,
    #[serde(default)]
    pub work_category: Option<String>, // "all" | "paper" | "book"
}

impl Default for HybridSearchOptions {
    fn default() -> Self {
        Self {
            top_n_per_branch: 50,
            rerank_enabled: true,
            rerank_top_m: 25,
            top_k_docs: 30,
            min_score: 0.0,
            rrf_k: DEFAULT_RRF_K,
            aggregation: DocScoreAggregation::Max,
            work_category: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchStats {
    /// Mode used: `"bm25_vector_rerank"`, `"bm25_vector"`, or `"bm25_only"`.
    pub mode: String,
    pub bm25_hits: usize,
    pub vector_hits: usize,
    pub rrf_candidates: usize,
    pub reranked_count: usize,
    pub total_ms: f64,
    pub bm25_ms: f64,
    pub embed_query_ms: f64,
    pub vector_ms: f64,
    pub rerank_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchResponse {
    pub results: Vec<ContentSearchDocResult>,
    pub stats: HybridSearchStats,
}

/// Optional ONNX + Vector runtime inputs for hybrid search.
/// Passing `None` gracefully degrades to BM25-only search.
pub struct HybridSearchContext<'a, FE, FR>
where
    FE: FnOnce() -> Result<OnnxEmbedder>,
    FR: FnOnce() -> Result<OnnxReranker>,
{
    pub vector_index: Option<&'a VectorIndex>,
    pub embedder_slot: Option<&'a LazyModelSlot<OnnxEmbedder>>,
    pub embedder_loader: Option<FE>,
    pub reranker_slot: Option<&'a LazyModelSlot<OnnxReranker>>,
    pub reranker_loader: Option<FR>,
}

/// Executes hybrid content search:
/// 1) BM25 (Tantivy) + Vector (USearch mmap) in parallel
/// 2) Reciprocal Rank Fusion (`1 / (k + rank)`) with `k = 60.0`
/// 3) Optional Cross-Encoder Reranking on top-M passages
/// 4) Document score aggregation (`Max` or `Top2Mean`) + cutoff/threshold
pub fn hybrid_search<FE, FR>(
    conn: &Connection,
    bm25: &Bm25Index,
    query: &str,
    root_ids: Option<&[String]>,
    opts: &HybridSearchOptions,
    mut ctx: Option<HybridSearchContext<'_, FE, FR>>,
) -> Result<HybridSearchResponse>
where
    FE: FnOnce() -> Result<OnnxEmbedder>,
    FR: FnOnce() -> Result<OnnxReranker>,
{
    let t_start = Instant::now();
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(HybridSearchResponse {
            results: Vec::new(),
            stats: HybridSearchStats {
                mode: "empty".to_string(),
                bm25_hits: 0,
                vector_hits: 0,
                rrf_candidates: 0,
                reranked_count: 0,
                total_ms: 0.0,
                bm25_ms: 0.0,
                embed_query_ms: 0.0,
                vector_ms: 0.0,
                rerank_ms: 0.0,
            },
        });
    }

    let top_n = opts.top_n_per_branch.max(1);
    let cat_filter = match opts
        .work_category
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("book") => Some("book"),
        Some("paper") => Some("paper"),
        _ => None,
    };

    let vector_can_run = ctx
        .as_ref()
        .is_some_and(|c| c.vector_index.is_some_and(|v| !v.is_empty()) && c.embedder_slot.is_some());

    // Only build the compact passage bitset when the vector branch will actually run with a filter!
    let allowed_pids = if vector_can_run && (root_ids.is_some() || cat_filter.is_some()) {
        Some(fetch_allowed_passage_ids(conn, root_ids, cat_filter)?)
    } else {
        None
    };

    let bm25_fetch_limit = if cat_filter.is_some() {
        (top_n * 6).max(300)
    } else {
        top_n
    };

    // Run BM25 and Vector embedding+search concurrently via scoped threads.
    let (bm25_res, vec_outcome) = std::thread::scope(|scope| {
        let bm25_handle = scope.spawn(|| {
            let t0 = Instant::now();
            let hits = bm25.search(trimmed, root_ids, bm25_fetch_limit);
            (hits, t0.elapsed().as_secs_f64() * 1000.0)
        });

        let vec_res = run_vector_branch(
            trimmed,
            top_n,
            allowed_pids.as_ref(),
            ctx.as_mut(),
        );
        let bm25_out = match bm25_handle.join() {
            Ok(out) => out,
            Err(_) => (Ok(Vec::new()), 0.0),
        };
        (bm25_out, vec_res)
    });

    let (mut bm25_hits, bm25_ms) = (bm25_res.0?, bm25_res.1);
    if let Some(cat) = cat_filter {
        if let Some(ref allowed) = allowed_pids {
            bm25_hits.retain(|h| allowed.contains(h.passage_id));
            bm25_hits.truncate(top_n);
        } else if !bm25_hits.is_empty() {
            // Filter the <= 300 BM25 hits by document `work_type` without scanning all 200k passages!
            let allowed_docs = fetch_matching_doc_ids_for_hits(conn, &bm25_hits, cat)?;
            bm25_hits.retain(|h| allowed_docs.contains(&(h.document_id as i64)));
            bm25_hits.truncate(top_n);
        }
    }
    let (vector_hits, embed_query_ms, vector_ms, vector_active) = vec_outcome;

    let mut fused = fuse_rrf(&bm25_hits, &vector_hits, opts.rrf_k.max(1.0));
    if fused.is_empty() {
        return Ok(HybridSearchResponse {
            results: Vec::new(),
            stats: HybridSearchStats {
                mode: if vector_active {
                    "bm25_vector".into()
                } else {
                    "bm25_only".into()
                },
                bm25_hits: bm25_hits.len(),
                vector_hits: vector_hits.len(),
                rrf_candidates: 0,
                reranked_count: 0,
                total_ms: t_start.elapsed().as_secs_f64() * 1000.0,
                bm25_ms,
                embed_query_ms,
                vector_ms,
                rerank_ms: 0.0,
            },
        });
    }

    // Hydrate candidate passages once from SQLite.
    let pids: Vec<u64> = fused.iter().map(|c| c.passage_id).collect();
    let hydrated = hydrate_passages_by_ids(conn, &pids)?;

    // Apply optional cross-encoder reranking on top-M RRF candidates when vector/reranker exist.
    let mut reranked_count = 0usize;
    let mut rerank_ms = 0.0f64;
    let rerank_cap = opts.rerank_top_m.min(fused.len());

    if opts.rerank_enabled && rerank_cap > 0 {
        if let Some(c) = ctx.as_mut() {
            if let Some(slot) = c.reranker_slot {
                let loader = c.reranker_loader.take();
                let mut pair_indices = Vec::with_capacity(rerank_cap);
                let mut pair_texts: Vec<&str> = Vec::with_capacity(rerank_cap);
                for (idx, cand) in fused.iter().take(rerank_cap).enumerate() {
                    if let Some(row) = hydrated.get(&cand.passage_id) {
                        pair_indices.push(idx);
                        pair_texts.push(row.text_content.as_str());
                    }
                }
                if !pair_texts.is_empty() {
                    let t_rerank = Instant::now();
                    let rerank_scores = slot.with_model(
                        || match loader {
                            Some(f) => f(),
                            None => Err(crate::error::CoreError::Config(
                                "reranker loader unavailable".into(),
                            )),
                        },
                        |reranker| reranker.score_pairs(trimmed, &pair_texts),
                    );
                    rerank_ms = t_rerank.elapsed().as_secs_f64() * 1000.0;
                    if let Ok(scores) = rerank_scores {
                        reranked_count = scores.len();
                        for (pos, &cand_idx) in pair_indices.iter().enumerate() {
                            // Boost reranked scores above un-reranked tail while preserving [0, 1] scale
                            fused[cand_idx].final_score = 1.0 + scores[pos];
                        }
                    }
                }
            }
        }
    }

    // Normalize RRF-only scores when no rerank ran, or preserve monotonic order when reranked tail remains.
    if reranked_count == 0 {
        let max_rrf = fused
            .first()
            .map(|c| c.rrf_score)
            .unwrap_or(1.0)
            .max(1e-9);
        for c in &mut fused {
            c.final_score = c.rrf_score / max_rrf;
        }
    } else {
        for c in &mut fused {
            if c.final_score >= 1.0 {
                c.final_score -= 1.0; // back to sigmoid [0, 1]
            } else {
                // Un-reranked tail stays strictly below reranked hits
                c.final_score = (c.rrf_score * 0.25).min(0.05);
            }
        }
    }

    let doc_results = aggregate_and_sort_documents(&fused, &hydrated, opts);
    let mode = if reranked_count > 0 && vector_active {
        "bm25_vector_rerank"
    } else if reranked_count > 0 {
        "bm25_rerank"
    } else if vector_active {
        "bm25_vector"
    } else {
        "bm25_only"
    };

    Ok(HybridSearchResponse {
        results: doc_results,
        stats: HybridSearchStats {
            mode: mode.to_string(),
            bm25_hits: bm25_hits.len(),
            vector_hits: vector_hits.len(),
            rrf_candidates: fused.len(),
            reranked_count,
            total_ms: t_start.elapsed().as_secs_f64() * 1000.0,
            bm25_ms,
            embed_query_ms,
            vector_ms,
            rerank_ms,
        },
    })
}

struct PassageBitSet {
    words: Vec<u64>,
}

impl PassageBitSet {
    fn new() -> Self {
        Self { words: Vec::new() }
    }

    #[inline]
    fn insert(&mut self, pid: u64) {
        let word_idx = (pid >> 6) as usize;
        if word_idx >= self.words.len() {
            // Avoid absurd allocations if a corrupt ID ever exceeded reasonable bounds
            if word_idx > 16_777_216 {
                return;
            }
            self.words.resize((word_idx + 1).next_power_of_two().max(64), 0);
        }
        self.words[word_idx] |= 1u64 << (pid & 63);
    }

    #[inline]
    fn contains(&self, pid: u64) -> bool {
        let word_idx = (pid >> 6) as usize;
        self.words
            .get(word_idx)
            .is_some_and(|&w| (w & (1u64 << (pid & 63))) != 0)
    }
}

fn run_vector_branch<FE, FR>(
    query: &str,
    top_n: usize,
    allowed_pids: Option<&PassageBitSet>,
    ctx: Option<&mut HybridSearchContext<'_, FE, FR>>,
) -> (Vec<VectorHit>, f64, f64, bool)
where
    FE: FnOnce() -> Result<OnnxEmbedder>,
    FR: FnOnce() -> Result<OnnxReranker>,
{
    let Some(c) = ctx else {
        return (Vec::new(), 0.0, 0.0, false);
    };
    let (Some(vindex), Some(slot)) = (c.vector_index, c.embedder_slot) else {
        return (Vec::new(), 0.0, 0.0, false);
    };
    if vindex.is_empty() {
        return (Vec::new(), 0.0, 0.0, false);
    }

    let loader = c.embedder_loader.take();
    let t_emb = Instant::now();
    let query_vec = match slot.with_model(
        || match loader {
            Some(f) => f(),
            None => Err(crate::error::CoreError::Config(
                "embedder loader unavailable".into(),
            )),
        },
        |emb| emb.embed_query(query),
    ) {
        Ok(v) if v.len() == vindex.dimensions() => v,
        _ => return (Vec::new(), t_emb.elapsed().as_secs_f64() * 1000.0, 0.0, false),
    };
    let embed_ms = t_emb.elapsed().as_secs_f64() * 1000.0;

    let t_vec = Instant::now();
    let hits = match allowed_pids {
        Some(allowed) => vindex
            .filtered_search(&query_vec, top_n, |pid| allowed.contains(pid))
            .unwrap_or_default(),
        None => vindex.search(&query_vec, top_n).unwrap_or_default(),
    };
    let vector_ms = t_vec.elapsed().as_secs_f64() * 1000.0;
    (hits, embed_ms, vector_ms, true)
}

#[derive(Debug, Clone)]
pub(crate) struct RrfCandidate {
    pub passage_id: u64,
    pub rrf_score: f32,
    pub final_score: f32,
}

/// Pure Reciprocal Rank Fusion (`sum_r 1 / (k + rank_r)` where `rank` is 1-based).
pub(crate) fn fuse_rrf(bm25_hits: &[Bm25Hit], vector_hits: &[VectorHit], k: f32) -> Vec<RrfCandidate> {
    let mut map: HashMap<u64, f32> = HashMap::new();
    for (idx, h) in bm25_hits.iter().enumerate() {
        let contrib = 1.0 / (k + (idx as f32 + 1.0));
        *map.entry(h.passage_id).or_insert(0.0) += contrib;
    }
    for (idx, h) in vector_hits.iter().enumerate() {
        let contrib = 1.0 / (k + (idx as f32 + 1.0));
        *map.entry(h.passage_id).or_insert(0.0) += contrib;
    }
    let mut out: Vec<RrfCandidate> = map
        .into_iter()
        .map(|(passage_id, rrf_score)| RrfCandidate {
            passage_id,
            rrf_score,
            final_score: rrf_score,
        })
        .collect();
    out.sort_by(|a, b| {
        b.rrf_score
            .partial_cmp(&a.rrf_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.passage_id.cmp(&b.passage_id))
    });
    out
}

pub(crate) struct HydratedPassage {
    pub passage_id: u64,
    pub document_id: i64,
    pub file_id: i64,
    pub page_ord: u32,
    pub printed_label: Option<String>,
    pub text_content: String,
    pub bboxes_blob: Vec<u8>,
    pub work_type: String,
    pub title: String,
    pub format: String,
    pub rel_path: String,
    pub is_available: bool,
}

fn fetch_matching_doc_ids_for_hits(
    conn: &Connection,
    hits: &[Bm25Hit],
    work_category: &str,
) -> Result<HashSet<i64>> {
    let mut doc_ids: Vec<i64> = hits.iter().map(|h| h.document_id as i64).collect();
    doc_ids.sort_unstable();
    doc_ids.dedup();
    if doc_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let cat_clause = match work_category {
        "book" => "work_type = 'book'",
        "paper" => "work_type != 'book'",
        _ => "1 = 1",
    };
    let mut allowed = HashSet::with_capacity(doc_ids.len());
    for chunk in doc_ids.chunks(500) {
        let placeholders = chunk
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT id FROM documents WHERE id IN ({placeholders}) AND {cat_clause}");
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> =
            chunk.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, i64>(0))?;
        for r in rows {
            allowed.insert(r?);
        }
    }
    Ok(allowed)
}

fn fetch_allowed_passage_ids(
    conn: &Connection,
    roots: Option<&[String]>,
    work_category: Option<&str>,
) -> Result<PassageBitSet> {
    if let Some(r) = roots {
        if r.is_empty() {
            return Ok(PassageBitSet::new());
        }
    }
    let cat_clause = match work_category {
        Some("book") => "d.work_type = 'book'",
        Some("paper") => "d.work_type != 'book'",
        _ => "1 = 1",
    };

    let mut set = PassageBitSet::new();
    if let Some(root_list) = roots {
        let placeholders = root_list
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        // Uses covering index `idx_passages_file_page(file_id, page_ord)` without reading passage heap pages!
        let sql = format!(
            "SELECT p.id FROM passages p
             WHERE p.file_id IN (
                 SELECT f.id FROM files f
                 JOIN documents d ON d.id = f.document_id
                 WHERE f.root_id IN ({placeholders}) AND {cat_clause}
             )"
        );
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> =
            root_list.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(refs.as_slice(), |r| r.get::<_, i64>(0))?;
        for r in rows {
            set.insert(r? as u64);
        }
    } else {
        // Uses covering index `idx_passages_doc(document_id)` without reading passage heap pages!
        let sql = format!(
            "SELECT p.id FROM passages p
             WHERE p.document_id IN (SELECT d.id FROM documents d WHERE {cat_clause})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        for r in rows {
            set.insert(r? as u64);
        }
    }
    Ok(set)
}

fn hydrate_passages_by_ids(
    conn: &Connection,
    pids: &[u64],
) -> Result<HashMap<u64, HydratedPassage>> {
    if pids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut out = HashMap::with_capacity(pids.len());
    // Chunk IN(...) queries safely under SQLite parameter limits.
    for chunk in pids.chunks(500) {
        let placeholders = chunk
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT p.id, p.document_id, p.file_id, p.page_ord, p.printed_label,
                    p.text_content, p.bboxes_blob, d.work_type, d.title, f.format, f.rel_path, f.is_available
             FROM passages p
             JOIN documents d ON d.id = p.document_id
             JOIN files f ON f.id = p.file_id
             WHERE p.id IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let ids: Vec<i64> = chunk.iter().map(|&id| id as i64).collect();
        let refs: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(refs.as_slice(), |r| {
            Ok(HydratedPassage {
                passage_id: r.get::<_, i64>(0)? as u64,
                document_id: r.get(1)?,
                file_id: r.get(2)?,
                page_ord: r.get::<_, i64>(3)? as u32,
                printed_label: r.get(4)?,
                text_content: r.get(5)?,
                bboxes_blob: r.get(6)?,
                work_type: r.get(7)?,
                title: r.get(8)?,
                format: r.get(9)?,
                rel_path: r.get(10)?,
                is_available: r.get::<_, i64>(11)? != 0,
            })
        })?;
        for row in rows {
            let hp = row?;
            out.insert(hp.passage_id, hp);
        }
    }
    Ok(out)
}

pub(crate) fn aggregate_and_sort_documents(
    candidates: &[RrfCandidate],
    hydrated: &HashMap<u64, HydratedPassage>,
    opts: &HybridSearchOptions,
) -> Vec<ContentSearchDocResult> {
    let mut grouped: HashMap<i64, ContentSearchDocResult> = HashMap::new();
    for cand in candidates {
        let Some(row) = hydrated.get(&cand.passage_id) else {
            continue;
        };
        let entry = grouped
            .entry(row.document_id)
            .or_insert_with(|| ContentSearchDocResult {
                document_id: row.document_id,
                file_id: row.file_id,
                work_type: row.work_type.clone(),
                title: row.title.clone(),
                format: row.format.clone(),
                rel_path: row.rel_path.clone(),
                is_available: row.is_available,
                max_score: 0.0,
                passages: Vec::new(),
            });
        entry.passages.push(ContentSearchPassageHit {
            passage_id: row.passage_id,
            file_id: row.file_id,
            page_ord: row.page_ord,
            printed_label: row.printed_label.clone(),
            score: cand.final_score,
            text_content: row.text_content.clone(),
            bboxes: unpack_bboxes(&row.bboxes_blob),
        });
    }

    let mut docs: Vec<ContentSearchDocResult> = grouped
        .into_values()
        .filter_map(|mut doc| {
            doc.passages.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            doc.max_score = match opts.aggregation {
                DocScoreAggregation::Max => doc.passages.first().map(|p| p.score).unwrap_or(0.0),
                DocScoreAggregation::Top2Mean => {
                    if doc.passages.len() >= 2 {
                        0.5 * (doc.passages[0].score + doc.passages[1].score)
                    } else {
                        doc.passages.first().map(|p| p.score).unwrap_or(0.0)
                    }
                }
            };
            if doc.max_score >= opts.min_score {
                Some(doc)
            } else {
                None
            }
        })
        .collect();

    docs.sort_by(|a, b| {
        b.max_score
            .partial_cmp(&a.max_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if opts.top_k_docs > 0 && docs.len() > opts.top_k_docs {
        docs.truncate(opts.top_k_docs);
    }
    docs
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
    use crate::index::bm25::Bm25Passage;
    use tempfile::tempdir;

    #[test]
    fn rrf_fusion_prioritizes_dual_branch_agreement() {
        let bm25 = vec![
            Bm25Hit { passage_id: 1, document_id: 1, file_id: 1, score: 5.0 },
            Bm25Hit { passage_id: 2, document_id: 2, file_id: 2, score: 4.0 },
        ];
        let vec = vec![
            VectorHit { passage_id: 3, score: 0.9 },
            VectorHit { passage_id: 2, score: 0.85 },
        ];
        let fused = fuse_rrf(&bm25, &vec, 60.0);
        // Passage 2 appears in both branches (rank 2 in both -> 2 / 62) > rank 1 only (1 / 61)
        assert_eq!(fused[0].passage_id, 2);
    }

    #[test]
    fn hybrid_search_degrades_gracefully_to_bm25_when_models_missing() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db"), 2).unwrap();
        let bm25 = Bm25Index::open_or_create(&dir.path().join("tantivy")).unwrap();

        let root_path = dir.path().join("docs");
        std::fs::create_dir_all(&root_path).unwrap();
        let root_id = "r1".to_string();

        let pids = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: root_id.clone(),
                    label: "Docs".into(),
                    root_path: root_path.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();
            let doc_id = create_heuristic_document(&w, "Banach Fixed Point Theorem").unwrap();
            let file_id = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: &root_id,
                    rel_path: "banach.pdf",
                    format: "pdf",
                    size_bytes: 100,
                    mtime_ns: 0,
                    blake3_hash: "h1",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            replace_file_pages_and_passages(
                &w,
                file_id,
                1,
                true,
                None,
                &[FilePageInsert {
                    file_id,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    width_pts: 600.0,
                    height_pts: 800.0,
                    text_source: "pdfium".into(),
                }],
                &[PassageInsert {
                    document_id: doc_id,
                    file_id,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    chunk_index: 0,
                    is_alt_formula: false,
                    token_count: 10,
                    text_content: "A contraction mapping on a complete metric space has a unique fixed point.".into(),
                    bboxes_blob: Vec::new(),
                }],
            )
            .unwrap()
        };

        bm25.add_passages(&[Bm25Passage {
            passage_id: pids[0],
            document_id: 1,
            file_id: 1,
            root_id,
            text_content: "A contraction mapping on a complete metric space has a unique fixed point.".into(),
        }])
        .unwrap();
        bm25.commit().unwrap();

        type LoaderFn<T> = fn() -> Result<T>;
        let none_ctx: Option<HybridSearchContext<'_, LoaderFn<OnnxEmbedder>, LoaderFn<OnnxReranker>>> = None;
        let resp = hybrid_search(
            &db.reader(),
            &bm25,
            "contraction mapping",
            None,
            &HybridSearchOptions::default(),
            none_ctx,
        )
        .unwrap();

        assert_eq!(resp.stats.mode, "bm25_only");
        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].passages[0].passage_id, pids[0]);
    }

    #[test]
    fn hybrid_search_filters_by_work_category() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("cat.db"), 2).unwrap();
        let bm25 = Bm25Index::open_or_create(&dir.path().join("tantivy_cat")).unwrap();

        let root_path = dir.path().join("docs");
        std::fs::create_dir_all(&root_path).unwrap();
        let root_id = "r1".to_string();

        let (paper_doc_id, book_doc_id, paper_pid, book_pid) = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: root_id.clone(),
                    label: "Docs".into(),
                    root_path: root_path.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();

            let p_doc = create_heuristic_document(&w, "Sharp Sobolev Embedding Constants").unwrap();
            let p_file = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: p_doc,
                    root_id: &root_id,
                    rel_path: "paper.pdf",
                    format: "pdf",
                    size_bytes: 100,
                    mtime_ns: 0,
                    blake3_hash: "hp",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            let p_pids = replace_file_pages_and_passages(
                &w,
                p_file,
                1,
                true,
                None,
                &[FilePageInsert {
                    file_id: p_file,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    width_pts: 600.0,
                    height_pts: 800.0,
                    text_source: "pdfium".into(),
                }],
                &[PassageInsert {
                    document_id: p_doc,
                    file_id: p_file,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    chunk_index: 0,
                    is_alt_formula: false,
                    token_count: 10,
                    text_content: "We prove the critical Sobolev embedding theorem on bounded domains.".into(),
                    bboxes_blob: Vec::new(),
                }],
            )
            .unwrap();

            let b_doc = create_heuristic_document(&w, "Functional Analysis, Sobolev Spaces and PDEs").unwrap();
            w.execute("UPDATE documents SET work_type = 'book' WHERE id = ?1", [b_doc])
                .unwrap();
            let b_file = insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: b_doc,
                    root_id: &root_id,
                    rel_path: "brezis_book.pdf",
                    format: "pdf",
                    size_bytes: 200,
                    mtime_ns: 0,
                    blake3_hash: "hb",
                    index_state: "bm25_done",
                },
            )
            .unwrap();
            let b_pids = replace_file_pages_and_passages(
                &w,
                b_file,
                1,
                true,
                None,
                &[FilePageInsert {
                    file_id: b_file,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    width_pts: 600.0,
                    height_pts: 800.0,
                    text_source: "pdfium".into(),
                }],
                &[PassageInsert {
                    document_id: b_doc,
                    file_id: b_file,
                    page_ord: 1,
                    printed_label: Some("1".into()),
                    chunk_index: 0,
                    is_alt_formula: false,
                    token_count: 10,
                    text_content: "Chapter 9 presents the Sobolev embedding theorem and Rellich compactness.".into(),
                    bboxes_blob: Vec::new(),
                }],
            )
            .unwrap();

            (p_doc, b_doc, p_pids[0], b_pids[0])
        };

        bm25.add_passages(&[
            Bm25Passage {
                passage_id: paper_pid,
                document_id: paper_doc_id as u64,
                file_id: 1,
                root_id: root_id.clone(),
                text_content: "We prove the critical Sobolev embedding theorem on bounded domains.".into(),
            },
            Bm25Passage {
                passage_id: book_pid,
                document_id: book_doc_id as u64,
                file_id: 2,
                root_id,
                text_content: "Chapter 9 presents the Sobolev embedding theorem and Rellich compactness.".into(),
            },
        ])
        .unwrap();
        bm25.commit().unwrap();

        type LoaderFn<T> = fn() -> Result<T>;
        let none_ctx: Option<HybridSearchContext<'_, LoaderFn<OnnxEmbedder>, LoaderFn<OnnxReranker>>> = None;

        let all_resp = hybrid_search(
            &db.reader(),
            &bm25,
            "Sobolev embedding theorem",
            None,
            &HybridSearchOptions::default(),
            none_ctx,
        )
        .unwrap();
        assert_eq!(all_resp.results.len(), 2);

        let none_ctx_p: Option<HybridSearchContext<'_, LoaderFn<OnnxEmbedder>, LoaderFn<OnnxReranker>>> = None;
        let paper_resp = hybrid_search(
            &db.reader(),
            &bm25,
            "Sobolev embedding theorem",
            None,
            &HybridSearchOptions {
                work_category: Some("paper".into()),
                ..HybridSearchOptions::default()
            },
            none_ctx_p,
        )
        .unwrap();
        assert_eq!(paper_resp.results.len(), 1);
        assert_eq!(paper_resp.results[0].document_id, paper_doc_id);
        assert_eq!(paper_resp.results[0].work_type, "article");

        let none_ctx_b: Option<HybridSearchContext<'_, LoaderFn<OnnxEmbedder>, LoaderFn<OnnxReranker>>> = None;
        let book_resp = hybrid_search(
            &db.reader(),
            &bm25,
            "Sobolev embedding theorem",
            None,
            &HybridSearchOptions {
                work_category: Some("book".into()),
                ..HybridSearchOptions::default()
            },
            none_ctx_b,
        )
        .unwrap();
        assert_eq!(book_resp.results.len(), 1);
        assert_eq!(book_resp.results[0].document_id, book_doc_id);
        assert_eq!(book_resp.results[0].work_type, "book");
    }
}
