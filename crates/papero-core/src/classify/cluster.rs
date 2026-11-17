use crate::classify::knn::{compute_document_mean_vectors, dot, DocMeanVector};
use crate::db::Database;
use crate::error::Result;
use crate::index::vector::VectorIndex;
use crate::ml::embedder::l2_normalize;
use crate::ml::ModelProfile;
use crate::text_norm::normalize_math_text;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicClusterView {
    pub id: i64,
    pub model_profile: String,
    pub label: String,
    pub is_label_locked: bool,
    pub keywords: Vec<String>,
    pub document_count: usize,
    pub document_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusteringReport {
    pub documents_clustered: usize,
    pub clusters_created: usize,
    pub selected_k: usize,
    pub silhouette_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagSummary {
    pub id: i64,
    pub name: String,
    pub color_hex: Option<String>,
    pub document_count: usize,
}

/// Runs deterministic spherical k-means++ with automatic Silhouette K-selection (when
/// `requested_k` is `None`) and c-TF-IDF keyword cluster labels, preserving manually
/// locked cluster labels and user-assigned cluster memberships (`is_user_assigned = 1`).
/// Runs spherical k-means++ using a read-only connection for vector loading & Silhouette
/// selection, acquiring the writer lock only for the brief SQLite write transaction.
pub fn run_topic_clustering_on_db(
    db: &Database,
    vindex: &VectorIndex,
    profile: ModelProfile,
    requested_k: Option<usize>,
) -> Result<ClusteringReport> {
    let docs = {
        let r = db.reader();
        compute_document_mean_vectors(&r, vindex)?
    };
    compute_and_save_clusters_from_docs(docs, profile, requested_k, |cb| {
        let w = db.writer();
        cb(&w)
    })
}

pub fn run_topic_clustering(
    conn: &Connection,
    vindex: &VectorIndex,
    profile: ModelProfile,
    requested_k: Option<usize>,
) -> Result<ClusteringReport> {
    let docs = compute_document_mean_vectors(conn, vindex)?;
    compute_and_save_clusters_from_docs(docs, profile, requested_k, |cb| cb(conn))
}

fn compute_and_save_clusters_from_docs<F>(
    docs: Vec<DocMeanVector>,
    profile: ModelProfile,
    requested_k: Option<usize>,
    with_writer: F,
) -> Result<ClusteringReport>
where
    F: FnOnce(&dyn Fn(&Connection) -> Result<()>) -> Result<()>,
{
    let n = docs.len();
    if n < 2 {
        return Ok(ClusteringReport {
            documents_clustered: n,
            clusters_created: 0,
            selected_k: 0,
            silhouette_score: 0.0,
        });
    }

    let vectors: Vec<&[f32]> = docs.iter().map(|d| d.vector.as_slice()).collect();
    let (selected_k, mut assignments, silhouette) = if let Some(k) = requested_k {
        let clamped = k.clamp(1, n);
        let (assigns, _) = spherical_kmeans_pp(&vectors, clamped, 42);
        let sil = if clamped >= 2 && clamped < n {
            mean_cosine_silhouette(&vectors, &assigns, clamped)
        } else {
            0.0
        };
        (clamped, assigns, sil)
    } else {
        let max_k = ((n as f64).sqrt().ceil() as usize + 2).min(16).min(n - 1).max(2);
        let mut best_k = 2usize;
        let mut best_assigns = Vec::new();
        let mut best_sil = f32::NEG_INFINITY;
        for cand_k in 2..=max_k {
            let (assigns, _) = spherical_kmeans_pp(&vectors, cand_k, 42);
            let sil = mean_cosine_silhouette(&vectors, &assigns, cand_k);
            if sil > best_sil {
                best_sil = sil;
                best_k = cand_k;
                best_assigns = assigns;
            }
        }
        (best_k, best_assigns, best_sil)
    };

    let mut active_indices: Vec<usize> = Vec::new();
    let mut remap: HashMap<usize, usize> = HashMap::new();
    for &c in &assignments {
        if let std::collections::hash_map::Entry::Vacant(e) = remap.entry(c) {
            let next = active_indices.len();
            e.insert(next);
            active_indices.push(c);
        }
    }
    for c in &mut assignments {
        *c = remap[c];
    }
    let cluster_count = active_indices.len();

    let keywords_by_cluster = extract_ctfidf_keywords(&docs, &assignments, cluster_count);
    let now = chrono::Utc::now().timestamp();
    let prof_str = profile.as_str();

    with_writer(&|conn: &Connection| {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM document_clusters
             WHERE is_user_assigned = 0
               AND cluster_id IN (SELECT id FROM topic_clusters WHERE model_profile = ?1)",
            params![prof_str],
        )?;
        tx.execute(
            "DELETE FROM topic_clusters
             WHERE model_profile = ?1
               AND is_label_locked = 0
               AND id NOT IN (SELECT DISTINCT cluster_id FROM document_clusters WHERE is_user_assigned = 1)",
            params![prof_str],
        )?;

        let mut cluster_db_ids = vec![0_i64; cluster_count];
        for c_idx in 0..cluster_count {
            let kws = &keywords_by_cluster[c_idx];
            let label = if kws.is_empty() {
                format!("Gruppo tematico {}", c_idx + 1)
            } else {
                kws.join(" · ")
            };
            let kws_json = serde_json::to_string(kws)?;
            tx.execute(
                "INSERT INTO topic_clusters (model_profile, label, is_label_locked, keywords_json, updated_at)
                 VALUES (?1, ?2, 0, ?3, ?4)",
                params![prof_str, label, kws_json, now],
            )?;
            cluster_db_ids[c_idx] = tx.last_insert_rowid();
        }

        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO document_clusters (document_id, cluster_id, is_user_assigned, score)
                 VALUES (?1, ?2, 0, 1.0)",
            )?;
            for (doc, &c_idx) in docs.iter().zip(assignments.iter()) {
                let cid = cluster_db_ids[c_idx];
                stmt.execute(params![doc.document_id, cid])?;
            }
        }

        tx.commit()?;
        Ok(())
    })?;

    Ok(ClusteringReport {
        documents_clustered: n,
        clusters_created: cluster_count,
        selected_k,
        silhouette_score: silhouette,
    })
}

pub fn list_topic_clusters(conn: &Connection) -> Result<Vec<TopicClusterView>> {
    let mut stmt = conn.prepare(
        "SELECT id, model_profile, label, is_label_locked, keywords_json
         FROM topic_clusters
         ORDER BY label COLLATE NOCASE ASC, id ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)? != 0,
            r.get::<_, String>(4)?,
        ))
    })?;

    let mut member_stmt =
        conn.prepare("SELECT cluster_id, document_id FROM document_clusters ORDER BY document_id ASC")?;
    let mrows = member_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    let mut by_cluster: HashMap<i64, Vec<i64>> = HashMap::new();
    for m in mrows {
        let (cid, did) = m?;
        by_cluster.entry(cid).or_default().push(did);
    }

    let mut out = Vec::new();
    for r in rows {
        let (id, model_profile, label, is_label_locked, kws_json) = r?;
        let keywords: Vec<String> = serde_json::from_str(&kws_json).unwrap_or_default();
        let document_ids = by_cluster.remove(&id).unwrap_or_default();
        let document_count = document_ids.len();
        out.push(TopicClusterView {
            id,
            model_profile,
            label,
            is_label_locked,
            keywords,
            document_count,
            document_ids,
        });
    }
    Ok(out)
}

pub fn rename_topic_cluster(conn: &Connection, cluster_id: i64, new_label: &str) -> Result<()> {
    let trimmed = new_label.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE topic_clusters SET label = ?1, is_label_locked = 1, updated_at = ?2 WHERE id = ?3",
        params![trimmed, now, cluster_id],
    )?;
    Ok(())
}

pub fn set_document_cluster(conn: &Connection, document_id: i64, cluster_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO document_clusters (document_id, cluster_id, is_user_assigned, score)
         VALUES (?1, ?2, 1, 1.0)
         ON CONFLICT(document_id, cluster_id) DO UPDATE SET is_user_assigned = 1",
        params![document_id, cluster_id],
    )?;
    Ok(())
}

pub fn remove_document_from_cluster(
    conn: &Connection,
    document_id: i64,
    cluster_id: i64,
) -> Result<()> {
    conn.execute(
        "DELETE FROM document_clusters WHERE document_id = ?1 AND cluster_id = ?2",
        params![document_id, cluster_id],
    )?;
    Ok(())
}

pub fn list_all_tags(conn: &Connection) -> Result<Vec<TagSummary>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.name, t.color_hex, COUNT(dt.document_id) AS cnt
         FROM tags t
         LEFT JOIN document_tags dt ON dt.tag_id = t.id
         GROUP BY t.id
         ORDER BY t.name COLLATE NOCASE ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TagSummary {
            id: r.get(0)?,
            name: r.get(1)?,
            color_hex: r.get(2)?,
            document_count: r.get::<_, i64>(3)? as usize,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn add_tag_to_document(conn: &Connection, document_id: i64, tag_name: &str) -> Result<()> {
    let trimmed = tag_name.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    conn.execute(
        "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
        params![trimmed],
    )?;
    let tag_id: i64 = conn.query_row(
        "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
        params![trimmed],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO document_tags (document_id, tag_id) VALUES (?1, ?2)",
        params![document_id, tag_id],
    )?;
    Ok(())
}

pub fn remove_tag_from_document(conn: &Connection, document_id: i64, tag_name: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM document_tags
         WHERE document_id = ?1
           AND tag_id IN (SELECT id FROM tags WHERE name = ?2 COLLATE NOCASE)",
        params![document_id, tag_name.trim()],
    )?;
    // Clean up orphaned tags cleanly
    conn.execute(
        "DELETE FROM tags WHERE id NOT IN (SELECT DISTINCT tag_id FROM document_tags)",
        [],
    )?;
    Ok(())
}

struct DeterministicRng(u64);

impl DeterministicRng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        (self.next_u64() as usize) % bound.max(1)
    }

    fn next_f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64) / ((1_u64 << 53) as f64)
    }
}

fn spherical_kmeans_pp(vectors: &[&[f32]], k: usize, seed: u64) -> (Vec<usize>, Vec<Vec<f32>>) {
    let n = vectors.len();
    let dim = vectors[0].len();
    let k = k.clamp(1, n);
    let mut rng = DeterministicRng(seed);

    // k-means++ seeding using cosine squared distance: d^2 = 2 * (1 - cos_sim)
    let mut centroids: Vec<Vec<f32>> = Vec::with_capacity(k);
    let first_idx = rng.next_usize(n);
    centroids.push(vectors[first_idx].to_vec());

    let mut min_dists = vec![f32::MAX; n];
    while centroids.len() < k {
        let last = centroids.last().unwrap();
        let mut total_w = 0.0f64;
        for (i, v) in vectors.iter().enumerate() {
            let d = (1.0 - dot(v, last)).max(0.0);
            if d < min_dists[i] {
                min_dists[i] = d;
            }
            total_w += f64::from(min_dists[i]);
        }
        let chosen = if total_w <= 1e-12 {
            rng.next_usize(n)
        } else {
            let mut target = rng.next_f64() * total_w;
            let mut idx = n - 1;
            for (i, &w) in min_dists.iter().enumerate() {
                target -= f64::from(w);
                if target <= 0.0 {
                    idx = i;
                    break;
                }
            }
            idx
        };
        centroids.push(vectors[chosen].to_vec());
    }

    let mut assignments = vec![0usize; n];
    for _iter in 0..25 {
        let mut changed = false;
        for (i, v) in vectors.iter().enumerate() {
            let mut best_c = 0usize;
            let mut best_sim = f32::NEG_INFINITY;
            for (c_idx, cent) in centroids.iter().enumerate() {
                let s = dot(v, cent);
                if s > best_sim {
                    best_sim = s;
                    best_c = c_idx;
                }
            }
            if assignments[i] != best_c {
                assignments[i] = best_c;
                changed = true;
            }
        }
        if !changed && _iter > 0 {
            break;
        }

        let mut sums = vec![vec![0.0f32; dim]; k];
        let mut counts = vec![0usize; k];
        for (i, v) in vectors.iter().enumerate() {
            let c = assignments[i];
            counts[c] += 1;
            for (a, val) in sums[c].iter_mut().zip(v.iter()) {
                *a += *val;
            }
        }
        for c in 0..k {
            if counts[c] == 0 {
                // Re-seed empty centroid to farthest point
                let mut worst_i = 0usize;
                let mut worst_sim = f32::INFINITY;
                for (i, v) in vectors.iter().enumerate() {
                    let s = dot(v, &centroids[assignments[i]]);
                    if s < worst_sim {
                        worst_sim = s;
                        worst_i = i;
                    }
                }
                centroids[c] = vectors[worst_i].to_vec();
            } else {
                l2_normalize(&mut sums[c]);
                centroids[c] = std::mem::take(&mut sums[c]);
            }
        }
    }

    (assignments, centroids)
}

fn mean_cosine_silhouette(vectors: &[&[f32]], assignments: &[usize], k: usize) -> f32 {
    let n = vectors.len();
    if k < 2 || n <= k {
        return 0.0;
    }
    // Cap silhouette sample deterministically at 300 points so large collections stay sub-second
    let step = (n / 300).max(1);
    let indices: Vec<usize> = (0..n).step_by(step).collect();

    let mut total_sil = 0.0f32;
    let mut valid_pts = 0usize;

    for &i in &indices {
        let own_c = assignments[i];
        let mut sum_dist_by_c = vec![0.0f32; k];
        let mut cnt_by_c = vec![0usize; k];

        for &j in &indices {
            if i == j {
                continue;
            }
            let c_j = assignments[j];
            let dist = (1.0 - dot(vectors[i], vectors[j])).max(0.0);
            sum_dist_by_c[c_j] += dist;
            cnt_by_c[c_j] += 1;
        }

        if cnt_by_c[own_c] == 0 {
            // By Rousseeuw (1987), singleton clusters have silhouette 0.0 (penalizes over-splitting)
            valid_pts += 1;
            continue;
        }
        let a_i = sum_dist_by_c[own_c] / (cnt_by_c[own_c] as f32);
        let mut b_i = f32::INFINITY;
        for c in 0..k {
            if c != own_c && cnt_by_c[c] > 0 {
                let mean_d = sum_dist_by_c[c] / (cnt_by_c[c] as f32);
                if mean_d < b_i {
                    b_i = mean_d;
                }
            }
        }
        if b_i.is_finite() {
            let denom = a_i.max(b_i);
            if denom > 1e-9 {
                total_sil += (b_i - a_i) / denom;
            }
            valid_pts += 1;
        }
    }

    if valid_pts == 0 {
        0.0
    } else {
        total_sil / (valid_pts as f32)
    }
}

fn extract_ctfidf_keywords(
    docs: &[DocMeanVector],
    assignments: &[usize],
    k: usize,
) -> Vec<Vec<String>> {
    let stopwords: HashSet<&'static str> = [
        "the", "and", "for", "with", "that", "this", "from", "are", "into", "over", "via", "using",
        "on", "of", "in", "to", "by", "an", "as", "is", "be", "or", "we", "let", "show", "prove",
        "proof", "theorem", "lemma", "proposition", "corollary", "definition", "section",
        "del", "della", "dei", "delle", "con", "per", "una", "uno", "gli", "nel", "nella", "sul",
        "sulla", "che", "come", "teorema", "lemma", "dimostrazione", "note", "notes", "paper",
        "introduction", "mathematics", "mathematical", "theory",
    ]
    .into_iter()
    .collect();

    let mut global_tf: HashMap<String, usize> = HashMap::new();
    let mut cluster_tf: Vec<HashMap<String, usize>> = vec![HashMap::new(); k];
    let mut cluster_total_terms = vec![0usize; k];
    let mut total_terms = 0usize;

    for (doc, &c_idx) in docs.iter().zip(assignments.iter()) {
        // Weight title terms 3x so descriptive monograph titles surface cleanly
        let combined = format!("{} {} {} {}", doc.title, doc.title, doc.title, doc.lead_text);
        let norm = normalize_math_text(&combined);
        for raw_tok in norm.split(|ch: char| !ch.is_alphanumeric()) {
            if raw_tok.len() < 3 || raw_tok.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if stopwords.contains(raw_tok) {
                continue;
            }
            let tok = raw_tok.to_string();
            *global_tf.entry(tok.clone()).or_insert(0) += 1;
            *cluster_tf[c_idx].entry(tok).or_insert(0) += 1;
            cluster_total_terms[c_idx] += 1;
            total_terms += 1;
        }
    }

    let avg_cluster_len = if k > 0 {
        (total_terms as f64 / k as f64).max(1.0)
    } else {
        1.0
    };

    let mut out = Vec::with_capacity(k);
    for c_idx in 0..k {
        let mut scored: Vec<(String, f64)> = cluster_tf[c_idx]
            .iter()
            .map(|(term, &tf_c)| {
                let tf_g = *global_tf.get(term).unwrap_or(&1) as f64;
                // Classic BERTopic c-TF-IDF formula
                let tf_norm = (tf_c as f64) / (cluster_total_terms[c_idx].max(1) as f64);
                let idf = (1.0 + avg_cluster_len / tf_g).ln();
                (term.clone(), tf_norm * idf)
            })
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        out.push(scored.into_iter().take(4).map(|(w, _)| w).collect());
    }
    out
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
    fn spherical_kmeans_separates_orthogonal_topics_and_preserves_user_edits() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("cluster.db"), 2).unwrap();
        let vindex = VectorIndex::open(&dir.path().join("vec.usearch"), 4).unwrap();
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();

        let mut docs_pids = Vec::new();
        {
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
            let specs = [
                ("Riemann Zeta Prime Asymptotics", "p1.pdf", [1.0, 0.05, 0.0, 0.0]),
                ("Euler Product Dirichlet Series", "p2.pdf", [0.99, -0.05, 0.0, 0.0]),
                ("Stochastic Brownian Martingale", "p3.pdf", [0.0, 0.0, 1.0, 0.04]),
                ("Doob Upcrossing Ergodic Process", "p4.pdf", [0.0, 0.0, 0.99, -0.04]),
            ];
            for (idx, (title, rel, vec)) in specs.iter().enumerate() {
                let did = create_heuristic_document(&w, title).unwrap();
                let fid = insert_file_record(
                    &w,
                    &NewFileRecord {
                        document_id: did,
                        root_id: "r1",
                        rel_path: rel,
                        format: "pdf",
                        size_bytes: 10,
                        mtime_ns: idx as i64,
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
                let mut v = vec.to_vec();
                l2_normalize(&mut v);
                docs_pids.push((did, pids[0], v));
            }
        }
        let batch: Vec<(u64, Vec<f32>)> = docs_pids
            .iter()
            .map(|(_, pid, v)| (*pid, v.clone()))
            .collect();
        vindex.upsert_batch(&batch).unwrap();

        let rep = run_topic_clustering(&db.writer(), &vindex, ModelProfile::Light, None).unwrap();
        assert_eq!(rep.documents_clustered, 4);
        assert_eq!(rep.clusters_created, 2);
        assert!(rep.silhouette_score > 0.7);

        let clusters = list_topic_clusters(&db.reader()).unwrap();
        assert_eq!(clusters.len(), 2);

        // Lock a renamed label and verify re-clustering preserves it
        rename_topic_cluster(&db.writer(), clusters[0].id, "Analytic Number Theory").unwrap();
        set_document_cluster(&db.writer(), docs_pids[0].0, clusters[0].id).unwrap();

        let _ = run_topic_clustering(&db.writer(), &vindex, ModelProfile::Light, Some(2)).unwrap();
        let after = list_topic_clusters(&db.reader()).unwrap();
        assert!(after
            .iter()
            .any(|c| c.is_label_locked && c.label == "Analytic Number Theory"));
    }
}
