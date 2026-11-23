use crate::db::Database;
use crate::dedup::minhash::{jaccard_similarity, unpack_minhash, MINHASH_LEN};
use crate::error::{CoreError, Result};
use crate::metadata::extract::{alternate_isbn_form, looks_like_garbage_title, normalize_isbn};
use crate::metadata::models::extract_arxiv_from_doi;
use crate::metadata::mrlookup::{
    compute_title_similarity, is_erratum_or_review_mismatch, normalize_title_words, pair_title_sim,
    significant_title_tokens,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use strsim::jaro_winkler;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateFileMember {
    pub document_id: i64,
    pub file_id: i64,
    pub root_id: String,
    pub root_label: String,
    pub rel_path: String,
    pub format: String,
    pub title: String,
    pub year: Option<i64>,
    pub edition: Option<String>,
    pub isbn: Option<String>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub mr_number: Option<String>,
    pub zbl_number: Option<String>,
    pub page_count: Option<u32>,
    pub size_bytes: u64,
    pub is_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateGroupView {
    pub group_id: i64,
    pub group_level: i64, // 1=identical_copies, 2=same_work_id, 3=probable_duplicate, 4=versions, 5=book_editions
    pub max_similarity: f32,
    pub reason_detail: String,
    pub status: String,
    pub members: Vec<DuplicateFileMember>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DuplicateReport {
    pub pairs_checked: usize,
    pub pairs_skipped_cached: usize,
    pub new_groups_created: usize,
}

#[derive(Debug, Clone)]
struct DocSig {
    id: i64,
    work_type: String,
    title: String,
    stripped_title: String,
    title_norm: String,
    sig_token_hashes: Vec<u64>,
    authors_norm: String,
    norm_series: Option<String>,
    norm_volume: Option<String>,
    title_vol_part: Option<u32>,
    is_garbage_title: bool,
    meaningful_title: bool,
    has_sub_sep: bool,
    has_erratum_marker: bool,
    year: Option<i64>,
    edition: Option<String>,
    isbn_canon: Option<String>,
    doi: Option<String>,
    arxiv_id: Option<String>,
    mr_number: Option<String>,
    zbl_number: Option<String>,
    is_heuristic: bool,
    minhash: Option<[u64; MINHASH_LEN]>,
    primary_file_id: i64,
    page_count: Option<u32>,
    primary_rel_path: String,
    pair_sig_u64: u64,
    pair_sig_hash: String,
}

#[derive(Debug, Default)]
struct CorpusStats {
    heuristic_title_counts: HashMap<String, usize>,
}

impl CorpusStats {
    fn from_docs(docs: &[DocSig]) -> Self {
        let mut heuristic_title_counts: HashMap<String, usize> = HashMap::new();
        for d in docs {
            if d.is_heuristic && !d.title_norm.is_empty() {
                *heuristic_title_counts.entry(d.title_norm.clone()).or_insert(0) += 1;
            }
        }
        Self {
            heuristic_title_counts,
        }
    }

    fn heuristic_title_freq(&self, title_norm: &str) -> usize {
        self.heuristic_title_counts
            .get(title_norm)
            .copied()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy)]
struct PairMatch {
    level: i64, // 2, 3, 4, or 5
    similarity: f32,
    reason: &'static str,
}

#[derive(Debug, Clone)]
struct PendingCluster {
    existing_gid: Option<i64>,
    level: i64,
    members: Vec<i64>,
    max_similarity: f32,
    reason: String,
    dirty: bool,
}

/// Runs incremental duplicate, preprint/published version, and book edition detection
/// without recomputing unchanged pairs already recorded in `duplicate_evaluated_pairs`.
pub fn run_duplicate_detection(db: &Database) -> Result<DuplicateReport> {
    let conn = db.writer();
    run_duplicate_detection_conn(&conn)
}

/// Refreshes `hash_sig_min` / `hash_sig_max` on user-resolved pairs (`'distinct'`, `'versions'`,
/// `'editions'`, `'merged'`) after a schema/signature-format upgrade so user decisions are preserved.
pub(crate) fn refresh_user_decided_pair_hashes(conn: &Connection) -> Result<()> {
    let sig_map: HashMap<i64, String> = load_doc_signatures(conn)?
        .into_iter()
        .map(|d| (d.id, d.pair_sig_hash))
        .collect();
    let pairs: Vec<(i64, i64)> = {
        let mut st = conn.prepare(
            "SELECT doc_id_min, doc_id_max FROM duplicate_evaluated_pairs
             WHERE decision NOT IN ('pending', 'clean')",
        )?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.filter_map(std::result::Result::ok).collect()
    };
    if pairs.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    {
        let mut upd = tx.prepare(
            "UPDATE duplicate_evaluated_pairs SET hash_sig_min = ?1, hash_sig_max = ?2
             WHERE doc_id_min = ?3 AND doc_id_max = ?4",
        )?;
        for (id_min, id_max) in pairs {
            if let (Some(h_min), Some(h_max)) = (sig_map.get(&id_min), sig_map.get(&id_max)) {
                upd.execute(params![h_min, h_max, id_min, id_max])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn run_duplicate_detection_conn(conn: &Connection) -> Result<DuplicateReport> {
    let now = chrono::Utc::now().timestamp();
    let mut report = DuplicateReport::default();

    ensure_level1_identical_copy_groups(conn, now)?;

    let docs = load_doc_signatures(conn)?;
    if docs.len() < 2 {
        conn.execute(
            "DELETE FROM duplicate_groups WHERE status = 'pending' AND group_level >= 2",
            [],
        )?;
        return Ok(report);
    }

    let stats = CorpusStats::from_docs(&docs);
    let doc_by_id: HashMap<i64, &DocSig> = docs.iter().map(|d| (d.id, d)).collect();

    // First, re-validate any existing unresolved ('pending') groups so stale false positives
    // (e.g. heuristic collana titles later resolved to distinct books) are pruned automatically.
    let mut clusters = revalidate_pending_groups(conn, &doc_by_id, &stats, now)?;

    let evaluated = load_evaluated_pairs(conn)?;
    let existing_active_pairs = load_existing_non_distinct_group_pairs(conn)?;

    let mut pair_lookup: HashMap<(i64, i64), PairMatch> = HashMap::new();
    for cl in &clusters {
        for i in 0..cl.members.len() {
            for j in (i + 1)..cl.members.len() {
                let (lo, hi) = ordered_ids(cl.members[i], cl.members[j]);
                if let (Some(da), Some(db_doc)) = (doc_by_id.get(&lo), doc_by_id.get(&hi)) {
                    if let Some(pm) = evaluate_pair(da, db_doc, &stats) {
                        pair_lookup.insert((lo, hi), pm);
                    }
                }
            }
        }
    }

    let candidate_indices = build_candidate_pair_indices(&docs);
    let mut clean_updates: Vec<(i64, i64, &str, &str)> = Vec::new();
    let mut new_matches: Vec<((i64, i64), PairMatch)> = Vec::new();

    for (idx_a, idx_b) in candidate_indices {
        let (a, b) = if docs[idx_a].id < docs[idx_b].id {
            (&docs[idx_a], &docs[idx_b])
        } else {
            (&docs[idx_b], &docs[idx_a])
        };

        if let Some(&(sig_min, sig_max, is_pending)) = evaluated.get(&(a.id, b.id)) {
            if sig_min == a.pair_sig_u64
                && sig_max == b.pair_sig_u64
                && (!is_pending || existing_active_pairs.contains(&(a.id, b.id)))
            {
                report.pairs_skipped_cached += 1;
                continue;
            }
        }

        report.pairs_checked += 1;
        let Some(matched) = evaluate_pair(a, b, &stats) else {
            clean_updates.push((a.id, b.id, &a.pair_sig_hash, &b.pair_sig_hash));
            continue;
        };

        pair_lookup.insert((a.id, b.id), matched);
        if existing_active_pairs.contains(&(a.id, b.id)) {
            continue;
        }
        new_matches.push(((a.id, b.id), matched));
    }

    // Sort matches deterministically so strongest matches form complete-linkage groups first,
    // avoiding C(K, 2) pairwise combinatorial group explosion when K >= 3 copies match.
    new_matches.sort_by(|(pa, ma), (pb, mb)| {
        ma.level
            .cmp(&mb.level)
            .then_with(|| {
                mb.similarity
                    .partial_cmp(&ma.similarity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| pa.cmp(pb))
    });

    for ((id_a, id_b), pm) in new_matches {
        if let Some(cl) = clusters
            .iter_mut()
            .find(|c| c.members.contains(&id_a) && c.members.contains(&id_b))
        {
            if pm.similarity > cl.max_similarity {
                cl.max_similarity = pm.similarity;
                cl.dirty = true;
            }
            continue;
        }

        if let Some(cl) = clusters
            .iter_mut()
            .find(|c| c.members.contains(&id_a) && can_join_cluster(c, id_b, pm.level, &pair_lookup))
        {
            cl.members.push(id_b);
            cl.max_similarity = cl.max_similarity.max(pm.similarity);
            cl.dirty = true;
            continue;
        }

        if let Some(cl) = clusters
            .iter_mut()
            .find(|c| c.members.contains(&id_b) && can_join_cluster(c, id_a, pm.level, &pair_lookup))
        {
            cl.members.push(id_a);
            cl.max_similarity = cl.max_similarity.max(pm.similarity);
            cl.dirty = true;
            continue;
        }

        clusters.push(PendingCluster {
            existing_gid: None,
            level: pm.level,
            members: vec![id_a, id_b],
            max_similarity: pm.similarity,
            reason: pm.reason.to_string(),
            dirty: true,
        });
    }

    let tx = conn.unchecked_transaction()?;
    {
        let mut clean_stmt = tx.prepare(
            "INSERT INTO duplicate_evaluated_pairs (doc_id_min, doc_id_max, hash_sig_min, hash_sig_max, decision, evaluated_at)
             VALUES (?1, ?2, ?3, ?4, 'clean', ?5)
             ON CONFLICT(doc_id_min, doc_id_max) DO UPDATE SET
                hash_sig_min = excluded.hash_sig_min,
                hash_sig_max = excluded.hash_sig_max,
                decision = 'clean',
                evaluated_at = excluded.evaluated_at",
        )?;
        for (id_min, id_max, h_min, h_max) in clean_updates {
            clean_stmt.execute(params![id_min, id_max, h_min, h_max, now])?;
        }
    }

    {
        let mut member_stmt = tx.prepare(
            "INSERT OR IGNORE INTO duplicate_group_members (group_id, document_id, file_id)
             VALUES (?1, ?2, ?3)",
        )?;
        let mut pending_pair_stmt = tx.prepare(
            "INSERT INTO duplicate_evaluated_pairs (doc_id_min, doc_id_max, hash_sig_min, hash_sig_max, decision, evaluated_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)
             ON CONFLICT(doc_id_min, doc_id_max) DO UPDATE SET
                hash_sig_min = excluded.hash_sig_min,
                hash_sig_max = excluded.hash_sig_max,
                decision = 'pending',
                evaluated_at = excluded.evaluated_at",
        )?;

        for cl in clusters {
            if !cl.dirty {
                continue;
            }
            let gid = match cl.existing_gid {
                Some(existing) => {
                    tx.execute(
                        "UPDATE duplicate_groups SET group_level = ?1, max_similarity = ?2, reason_detail = ?3
                         WHERE id = ?4",
                        params![cl.level, cl.max_similarity as f64, cl.reason, existing],
                    )?;
                    existing
                }
                None => {
                    tx.execute(
                        "INSERT INTO duplicate_groups (group_level, max_similarity, reason_detail, status, created_at)
                         VALUES (?1, ?2, ?3, 'pending', ?4)",
                        params![cl.level, cl.max_similarity as f64, cl.reason, now],
                    )?;
                    report.new_groups_created += 1;
                    tx.last_insert_rowid()
                }
            };

            for &doc_id in &cl.members {
                if let Some(d) = doc_by_id.get(&doc_id) {
                    member_stmt.execute(params![gid, d.id, d.primary_file_id])?;
                }
            }

            for i in 0..cl.members.len() {
                for j in (i + 1)..cl.members.len() {
                    let (lo, hi) = ordered_ids(cl.members[i], cl.members[j]);
                    if let (Some(da), Some(db_doc)) = (doc_by_id.get(&lo), doc_by_id.get(&hi)) {
                        pending_pair_stmt.execute(params![
                            da.id,
                            db_doc.id,
                            da.pair_sig_hash,
                            db_doc.pair_sig_hash,
                            now
                        ])?;
                    }
                }
            }
        }
    }

    tx.commit()?;
    Ok(report)
}

fn ordered_ids(a: i64, b: i64) -> (i64, i64) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

fn can_join_cluster(
    cluster: &PendingCluster,
    candidate_doc_id: i64,
    level: i64,
    pair_lookup: &HashMap<(i64, i64), PairMatch>,
) -> bool {
    const MAX_CLUSTER_MEMBERS: usize = 16;
    if cluster.level != level
        || cluster.members.len() >= MAX_CLUSTER_MEMBERS
        || cluster.members.contains(&candidate_doc_id)
    {
        return false;
    }
    cluster.members.iter().all(|&member_id| {
        let key = ordered_ids(member_id, candidate_doc_id);
        pair_lookup.get(&key).is_some_and(|pm| pm.level == level)
    })
}

fn revalidate_pending_groups(
    conn: &Connection,
    doc_by_id: &HashMap<i64, &DocSig>,
    stats: &CorpusStats,
    now: i64,
) -> Result<Vec<PendingCluster>> {
    let mut g_stmt = conn.prepare(
        "SELECT id, group_level, max_similarity, reason_detail
         FROM duplicate_groups
         WHERE status = 'pending' AND group_level >= 2
         ORDER BY id ASC",
    )?;
    let pending_groups: Vec<(i64, i64, f32, String)> = g_stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, f64>(2)? as f32,
                r.get(3)?,
            ))
        })?
        .filter_map(std::result::Result::ok)
        .collect();
    drop(g_stmt);

    if pending_groups.is_empty() {
        return Ok(Vec::new());
    }

    let mut m_stmt = conn.prepare(
        "SELECT dgm.group_id, dgm.document_id
         FROM duplicate_group_members dgm
         JOIN duplicate_groups dg ON dg.id = dgm.group_id
         WHERE dg.status = 'pending' AND dg.group_level >= 2
         ORDER BY dgm.group_id ASC, dgm.document_id ASC",
    )?;
    let mut members_by_gid: HashMap<i64, Vec<i64>> = HashMap::new();
    for row in m_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
        let (gid, doc_id) = row?;
        let entry = members_by_gid.entry(gid).or_default();
        if !entry.contains(&doc_id) {
            entry.push(doc_id);
        }
    }
    drop(m_stmt);

    let tx = conn.unchecked_transaction()?;
    let mut valid_clusters = Vec::new();

    for (gid, old_level, old_sim, old_reason) in pending_groups {
        let members = members_by_gid.remove(&gid).unwrap_or_default();
        let all_exist = members.len() >= 2 && members.iter().all(|id| doc_by_id.contains_key(id));
        let mut all_match = all_exist;
        let mut best_level: Option<i64> = None;
        let mut best_sim: f32 = 0.0;
        let mut best_reason: &'static str = "";

        if all_exist {
            'pairs: for i in 0..members.len() {
                for j in (i + 1)..members.len() {
                    let (lo, hi) = ordered_ids(members[i], members[j]);
                    let da = doc_by_id[&lo];
                    let db_doc = doc_by_id[&hi];
                    match evaluate_pair(da, db_doc, stats) {
                        Some(pm) => {
                            if let Some(lvl) = best_level {
                                if lvl != pm.level {
                                    all_match = false;
                                    break 'pairs;
                                }
                            } else {
                                best_level = Some(pm.level);
                                best_reason = pm.reason;
                            }
                            best_sim = best_sim.max(pm.similarity);
                        }
                        None => {
                            all_match = false;
                            break 'pairs;
                        }
                    }
                }
            }
        }

        if !all_match {
            tx.execute("DELETE FROM duplicate_groups WHERE id = ?1", params![gid])?;
            for i in 0..members.len() {
                for j in (i + 1)..members.len() {
                    let (lo, hi) = ordered_ids(members[i], members[j]);
                    if let (Some(da), Some(db_doc)) = (doc_by_id.get(&lo), doc_by_id.get(&hi)) {
                        if evaluate_pair(da, db_doc, stats).is_none() {
                            tx.execute(
                                "INSERT INTO duplicate_evaluated_pairs (doc_id_min, doc_id_max, hash_sig_min, hash_sig_max, decision, evaluated_at)
                                 VALUES (?1, ?2, ?3, ?4, 'clean', ?5)
                                 ON CONFLICT(doc_id_min, doc_id_max) DO UPDATE SET
                                    hash_sig_min = excluded.hash_sig_min,
                                    hash_sig_max = excluded.hash_sig_max,
                                    decision = 'clean',
                                    evaluated_at = excluded.evaluated_at",
                                params![da.id, db_doc.id, da.pair_sig_hash, db_doc.pair_sig_hash, now],
                            )?;
                        } else {
                            // Clear 'pending' cache so any surviving matching subset gets cleanly re-clustered
                            tx.execute(
                                "DELETE FROM duplicate_evaluated_pairs WHERE doc_id_min = ?1 AND doc_id_max = ?2 AND decision = 'pending'",
                                params![da.id, db_doc.id],
                            )?;
                        }
                    }
                }
            }
            continue;
        }

        let new_level = best_level.unwrap_or(old_level);
        let new_reason = if best_reason.is_empty() {
            old_reason.clone()
        } else {
            best_reason.to_string()
        };
        let dirty = new_level != old_level
            || (best_sim - old_sim).abs() > 0.005
            || new_reason != old_reason;

        valid_clusters.push(PendingCluster {
            existing_gid: Some(gid),
            level: new_level,
            members,
            max_similarity: best_sim,
            reason: new_reason,
            dirty,
        });
    }

    tx.commit()?;
    Ok(valid_clusters)
}

fn ensure_level1_identical_copy_groups(conn: &Connection, now: i64) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT f.document_id, f.blake3_hash, COUNT(f.id) AS cnt
         FROM files f
         GROUP BY f.document_id, f.blake3_hash
         HAVING cnt > 1",
    )?;
    let multi: Vec<(i64, String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(std::result::Result::ok)
        .collect();

    for (doc_id, hash, cnt) in multi {
        let already: Option<i64> = conn
            .query_row(
                "SELECT dg.id FROM duplicate_groups dg
                 JOIN duplicate_group_members dgm ON dgm.group_id = dg.id
                 WHERE dg.group_level = 1 AND dgm.document_id = ?1
                 LIMIT 1",
                params![doc_id],
                |r| r.get(0),
            )
            .optional()?;

        let gid = match already {
            Some(id) => id,
            None => {
                conn.execute(
                    "INSERT INTO duplicate_groups (group_level, max_similarity, reason_detail, status, created_at)
                     VALUES (1, 1.0, ?1, 'info_merged', ?2)",
                    params![format!("Copie identiche (BLAKE3 {}..., {cnt} file)", &hash[..12.min(hash.len())]), now],
                )?;
                conn.last_insert_rowid()
            }
        };

        let mut f_st = conn.prepare("SELECT id FROM files WHERE document_id = ?1")?;
        let f_ids: Vec<i64> = f_st
            .query_map(params![doc_id], |r| r.get(0))?
            .filter_map(std::result::Result::ok)
            .collect();
        for fid in f_ids {
            conn.execute(
                "INSERT OR IGNORE INTO duplicate_group_members (group_id, document_id, file_id)
                 VALUES (?1, ?2, ?3)",
                params![gid, doc_id, fid],
            )?;
        }
    }
    Ok(())
}

fn canonicalize_isbn(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    let norm = normalize_isbn(s).unwrap_or_else(|| {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .collect()
    });
    if norm.len() == 10 {
        alternate_isbn_form(&norm).or(Some(norm))
    } else if norm.is_empty() {
        None
    } else {
        Some(norm)
    }
}

fn minhash_short_tag(sig: Option<&[u64; MINHASH_LEN]>) -> u64 {
    let Some(s) = sig else {
        return 0;
    };
    let mut h: u64 = 0xcbf29ce484222325;
    for &v in s.iter().take(16) {
        h ^= v;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn load_doc_signatures(conn: &Connection) -> Result<Vec<DocSig>> {
    let mut stmt = conn.prepare(
        "SELECT d.id, d.work_type, d.title, d.title_norm, d.authors_norm_summary,
                d.series, d.volume, d.year, d.edition,
                d.isbn, d.doi, d.arxiv_id, d.mr_number, d.zbl_number,
                d.metadata_source, d.minhash_sig,
                MIN(f.id) AS primary_file_id, MIN(f.blake3_hash) AS primary_hash,
                MAX(f.page_count) AS page_count, MIN(f.rel_path) AS primary_rel_path
         FROM documents d
         JOIN files f ON f.document_id = d.id
         GROUP BY d.id",
    )?;
    let rows = stmt.query_map([], |r| {
        let work_type: String = r.get(1)?;
        let title: String = r.get(2)?;
        let title_norm: String = r.get(3)?;
        let authors_norm: String = r.get(4)?;
        let series: Option<String> = r.get(5)?;
        let volume: Option<String> = r.get(6)?;
        let year: Option<i64> = r.get(7)?;
        let edition: Option<String> = r.get(8)?;
        let raw_isbn: Option<String> = r.get(9)?;
        let isbn_canon = canonicalize_isbn(raw_isbn.as_deref());
        let doi: Option<String> = r.get(10)?;
        let arxiv_id: Option<String> = r.get(11)?;
        let mr_number: Option<String> = r.get(12)?;
        let zbl_number: Option<String> = r.get(13)?;
        let metadata_source: Option<String> = r.get(14)?;
        let mh_blob: Option<Vec<u8>> = r.get(15)?;
        let minhash = mh_blob.as_deref().and_then(unpack_minhash);
        let primary_file_id: i64 = r.get(16)?;
        let primary_hash: String = r.get(17)?;
        let page_count: Option<u32> = r.get::<_, Option<i64>>(18)?.map(|v| v as u32);
        let primary_rel_path: String = r.get(19)?;
        let is_heuristic = metadata_source.as_deref().unwrap_or("heuristic") == "heuristic";

        let pair_sig_u64 = fnv1a_doc_sig(&[
            &primary_hash,
            &work_type,
            &title_norm,
            &authors_norm,
            series.as_deref().unwrap_or(""),
            volume.as_deref().unwrap_or(""),
            &year.map(|y| y.to_string()).unwrap_or_default(),
            edition.as_deref().unwrap_or(""),
            isbn_canon.as_deref().unwrap_or(""),
            doi.as_deref().unwrap_or(""),
            arxiv_id.as_deref().unwrap_or(""),
            mr_number.as_deref().unwrap_or(""),
            zbl_number.as_deref().unwrap_or(""),
        ]) ^ minhash_short_tag(minhash.as_ref());
        let pair_sig_hash = format!("{pair_sig_u64:016x}");

        let stripped_title = strip_known_series_prefix(&title, series.as_deref());
        let is_garbage_title = looks_like_garbage_title(&title);
        let meaningful_title =
            is_meaningful_prose_title_prechecked(&title, &title_norm, is_garbage_title);
        let title_vol_part = extract_title_volume_or_part(&title);
        let norm_volume = normalize_volume_field(volume.as_deref());
        let norm_series = series
            .as_deref()
            .map(normalize_title_words)
            .filter(|s| !s.is_empty());
        let has_sub_sep = [" - ", " – ", " — ", ":"]
            .iter()
            .any(|sep| stripped_title.contains(sep));
        let has_erratum_marker = has_any_erratum_marker(&title_norm);
        let mut sig_token_hashes: Vec<u64> = significant_title_tokens(&title_norm)
            .into_iter()
            .map(fnv1a_str)
            .collect();
        sig_token_hashes.sort_unstable();
        sig_token_hashes.dedup();

        Ok(DocSig {
            id: r.get(0)?,
            work_type,
            title,
            stripped_title,
            title_norm,
            sig_token_hashes,
            authors_norm,
            norm_series,
            norm_volume,
            title_vol_part,
            is_garbage_title,
            meaningful_title,
            has_sub_sep,
            has_erratum_marker,
            year,
            edition,
            isbn_canon,
            doi,
            arxiv_id,
            mr_number,
            zbl_number,
            is_heuristic,
            minhash,
            primary_file_id,
            page_count,
            primary_rel_path,
            pair_sig_u64,
            pair_sig_hash,
        })
    })?;

    Ok(rows.filter_map(std::result::Result::ok).collect())
}

fn fnv1a_str(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in s.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn fnv1a_doc_sig(parts: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for &b in p.as_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    // Ensure 0 is reserved for unparseable legacy strings
    if h == 0 { 1 } else { h }
}

fn has_any_erratum_marker(norm: &str) -> bool {
    const MARKERS: &[&str] = &[
        "erratum",
        "errata",
        "corrigendum",
        "corrigenda",
        "correction",
        "retraction",
        "retracted",
        "addendum",
        "review",
    ];
    norm.split_whitespace().any(|w| MARKERS.contains(&w))
}

fn sorted_u64_f1(a: &[u64], b: &[u64]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut ia = 0usize;
    let mut ib = 0usize;
    let mut inter = 0usize;
    while ia < a.len() && ib < b.len() {
        match a[ia].cmp(&b[ib]) {
            std::cmp::Ordering::Equal => {
                inter += 1;
                ia += 1;
                ib += 1;
            }
            std::cmp::Ordering::Less => ia += 1,
            std::cmp::Ordering::Greater => ib += 1,
        }
    }
    (2 * inter) as f32 / (a.len() + b.len()) as f32
}

type EvaluatedPairMap = HashMap<(i64, i64), (u64, u64, bool)>;

fn load_evaluated_pairs(conn: &Connection) -> Result<EvaluatedPairMap> {
    let mut stmt = conn.prepare(
        "SELECT doc_id_min, doc_id_max, hash_sig_min, hash_sig_max, decision FROM duplicate_evaluated_pairs",
    )?;
    let rows = stmt.query_map([], |r| {
        let id_min: i64 = r.get(0)?;
        let id_max: i64 = r.get(1)?;
        let s_min = r.get_ref(2)?.as_str().unwrap_or("");
        let s_max = r.get_ref(3)?.as_str().unwrap_or("");
        let dec = r.get_ref(4)?.as_str().unwrap_or("");
        let h_min = if s_min.len() == 16 {
            u64::from_str_radix(s_min, 16).unwrap_or(0)
        } else {
            0
        };
        let h_max = if s_max.len() == 16 {
            u64::from_str_radix(s_max, 16).unwrap_or(0)
        } else {
            0
        };
        Ok(((id_min, id_max), (h_min, h_max, dec == "pending")))
    })?;
    let mut map = HashMap::new();
    for r in rows {
        let (k, v) = r?;
        map.insert(k, v);
    }
    Ok(map)
}

fn load_existing_non_distinct_group_pairs(conn: &Connection) -> Result<HashSet<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT m1.document_id, m2.document_id
         FROM duplicate_group_members m1
         JOIN duplicate_group_members m2 ON m1.group_id = m2.group_id AND m1.document_id < m2.document_id
         JOIN duplicate_groups dg ON dg.id = m1.group_id
         WHERE dg.status != 'distinct'",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.filter_map(std::result::Result::ok).collect())
}

fn is_generic_blocking_word(w: &str) -> bool {
    matches!(
        w,
        "mathematics"
            | "mathematical"
            | "matematico"
            | "matematica"
            | "matematiche"
            | "mathematik"
            | "mathematische"
            | "mathematiques"
            | "introduction"
            | "introduzione"
            | "elements"
            | "elementi"
            | "lectures"
            | "lecture"
            | "lezioni"
            | "notes"
            | "course"
            | "corso"
            | "theory"
            | "teoria"
            | "graduate"
            | "undergraduate"
            | "studies"
            | "monographs"
            | "series"
            | "collana"
            | "biblioteca"
            | "quaderni"
            | "scienza"
            | "scienze"
            | "mondo"
            | "volume"
            | "edition"
            | "edizione"
    )
}

fn build_candidate_pair_indices(docs: &[DocSig]) -> Vec<(usize, usize)> {
    let mut pairs: HashSet<(usize, usize)> = HashSet::new();
    if docs.len() <= 250 {
        for i in 0..docs.len() {
            for j in (i + 1)..docs.len() {
                pairs.insert((i, j));
            }
        }
        return pairs.into_iter().collect();
    }

    let mut exact_id_buckets: HashMap<String, Vec<usize>> = HashMap::new();
    let mut fuzzy_buckets: HashMap<String, Vec<usize>> = HashMap::new();

    for (idx, d) in docs.iter().enumerate() {
        for (prefix, val) in [
            ("doi:", d.doi.as_deref()),
            ("mr:", d.mr_number.as_deref()),
            ("zbl:", d.zbl_number.as_deref()),
            ("ax:", d.arxiv_id.as_deref()),
            ("isbn:", d.isbn_canon.as_deref()),
        ] {
            if let Some(v) = val {
                exact_id_buckets
                    .entry(format!("{prefix}{v}"))
                    .or_default()
                    .push(idx);
            }
        }

        // Distinctive title words (preferring non-generic tokens across the full title, not only prefix)
        let mut distinctive: Vec<&str> = d
            .title_norm
            .split_whitespace()
            .filter(|w| w.len() >= 5 && !is_generic_blocking_word(w))
            .collect();
        distinctive.sort_by_key(|w| std::cmp::Reverse(w.len()));
        distinctive.dedup();
        for w in distinctive.into_iter().take(5) {
            fuzzy_buckets
                .entry(format!("tw:{w}"))
                .or_default()
                .push(idx);
        }

        // 16-band LSH over the 128-slot body MinHash (8 u64s per band -> ~99.4% recall at Jaccard 0.85)
        if let Some(ref mh) = d.minhash {
            let (bands, _) = mh.as_chunks::<8>();
            for (band, slice) in bands.iter().enumerate() {
                let mut bh: u64 = 0xcbf29ce484222325;
                for &v in slice {
                    bh ^= v;
                    bh = bh.wrapping_mul(0x100000001b3);
                }
                fuzzy_buckets
                    .entry(format!("mh:{band}:{bh:016x}"))
                    .or_default()
                    .push(idx);
            }
        }
    }

    for idxs in exact_id_buckets.values() {
        if idxs.len() < 2 || idxs.len() > 32 {
            continue;
        }
        for a in 0..idxs.len() {
            for b in (a + 1)..idxs.len() {
                let (i, j) = if idxs[a] < idxs[b] {
                    (idxs[a], idxs[b])
                } else {
                    (idxs[b], idxs[a])
                };
                pairs.insert((i, j));
            }
        }
    }

    for idxs in fuzzy_buckets.values() {
        if idxs.len() < 2 || idxs.len() > 60 {
            continue;
        }
        for a in 0..idxs.len() {
            for b in (a + 1)..idxs.len() {
                let (i, j) = if idxs[a] < idxs[b] {
                    (idxs[a], idxs[b])
                } else {
                    (idxs[b], idxs[a])
                };
                pairs.insert((i, j));
            }
        }
    }

    pairs.into_iter().collect()
}

fn non_arxiv_doi(doi: Option<&str>) -> Option<&str> {
    let d = doi?;
    if d.starts_with("10.48550/") || extract_arxiv_from_doi(d).is_some() {
        None
    } else {
        Some(d)
    }
}

fn evaluate_pair(a: &DocSig, b: &DocSig, stats: &CorpusStats) -> Option<PairMatch> {
    // 0. Zero-allocation O(1) checks on IDs and body MinHash BEFORE any string/token work!
    let has_shared_arxiv = a.arxiv_id.is_some() && a.arxiv_id == b.arxiv_id;
    let same_strict_work_id = (a.doi.is_some() && a.doi == b.doi)
        || (a.mr_number.is_some() && a.mr_number == b.mr_number)
        || (a.zbl_number.is_some() && a.zbl_number == b.zbl_number)
        || has_shared_arxiv;
    let same_isbn = a.isbn_canon.is_some() && a.isbn_canon == b.isbn_canon;
    let text_sim = match (&a.minhash, &b.minhash) {
        (Some(sa), Some(sb)) => jaccard_similarity(sa, sb),
        _ => 0.0,
    };

    // Fast ~10ns zero-allocation reject for >99% of candidate pairs without shared IDs or high body similarity
    if !same_strict_work_id
        && !same_isbn
        && text_sim < 0.78
        && !a.has_sub_sep
        && !b.has_sub_sep
        && sorted_u64_f1(&a.sig_token_hashes, &b.sig_token_hashes) < 0.65
    {
        return None;
    }

    if (a.has_erratum_marker || b.has_erratum_marker)
        && is_erratum_or_review_mismatch(&a.title, &b.title)
    {
        return None;
    }

    let both_books = a.work_type == "book" && b.work_type == "book";
    let either_book = a.work_type == "book" || b.work_type == "book";

    let volume_part_conflict = has_conflicting_volume_or_part(a, b);
    if volume_part_conflict {
        return None;
    }

    // Preprint arXiv vs published journal version indicators
    let a_pub_doi = non_arxiv_doi(a.doi.as_deref());
    let b_pub_doi = non_arxiv_doi(b.doi.as_deref());
    let one_published_one_preprint = (a.arxiv_id.is_some()
        && b.arxiv_id.is_none()
        && (b_pub_doi.is_some() || b.mr_number.is_some() || b.zbl_number.is_some()))
        || (b.arxiv_id.is_some()
            && a.arxiv_id.is_none()
            && (a_pub_doi.is_some() || a.mr_number.is_some() || a.zbl_number.is_some()))
        || (has_shared_arxiv
            && ((a_pub_doi.is_some() ^ b_pub_doi.is_some())
                || (a.mr_number.is_some() ^ b.mr_number.is_some())));

    // 1. Exact shared article/work IDs (DOI / MR / Zbl / arXiv) -> Level 2 or Level 4
    if same_strict_work_id {
        if one_published_one_preprint {
            return Some(PairMatch {
                level: 4,
                similarity: 0.98,
                reason: "Versioni diverse dello stesso lavoro (preprint arXiv e versione pubblicata)",
            });
        }
        return Some(PairMatch {
            level: 2,
            similarity: 1.0,
            reason: "Stesso identificatore bibliografico (DOI / MR / Zbl / arXiv / ISBN)",
        });
    }

    let (title_sim, min_sig_words) = compute_dedup_title_similarity(a, b);
    let author_sim_opt = compute_dedup_author_similarity(&a.authors_norm, &b.authors_norm);

    // Shared ISBN -> Level 2 ONLY when not a shared multi-volume/set/collana ISBN on two distinct books
    if same_isbn {
        let distinct_real_books = a.meaningful_title
            && b.meaningful_title
            && title_sim < 0.45
            && (author_sim_opt.is_some_and(|s| s < 0.50)
                || !page_counts_compatible_for_duplicate(a.page_count, b.page_count));
        if !distinct_real_books {
            return Some(PairMatch {
                level: 2,
                similarity: 1.0,
                reason: "Stesso identificatore bibliografico (DOI / MR / Zbl / arXiv / ISBN)",
            });
        }
    }

    let different_doi = a_pub_doi.is_some() && b_pub_doi.is_some() && a_pub_doi != b_pub_doi;
    let different_mr = a.mr_number.is_some() && b.mr_number.is_some() && a.mr_number != b.mr_number;
    let different_zbl =
        a.zbl_number.is_some() && b.zbl_number.is_some() && a.zbl_number != b.zbl_number;
    let different_arxiv =
        a.arxiv_id.is_some() && b.arxiv_id.is_some() && a.arxiv_id != b.arxiv_id;
    let different_isbn =
        a.isbn_canon.is_some() && b.isbn_canon.is_some() && a.isbn_canon != b.isbn_canon;
    let different_edition = a.edition.is_some() && b.edition.is_some() && a.edition != b.edition;
    let different_year = a.year.is_some() && b.year.is_some() && a.year != b.year;

    let authors_compatible = author_sim_opt.is_none_or(|s| s >= 0.78);
    let authors_strong = author_sim_opt.is_some_and(|s| s >= 0.82);

    let title_author_same_work = (title_sim >= 0.93 && authors_strong && min_sig_words >= 2)
        || (title_sim >= 0.96
            && authors_compatible
            && min_sig_words >= 4
            && a.title_norm.len() >= 22
            && b.title_norm.len() >= 22);

    // 2. Level 4: Preprint arXiv <-> Published journal version
    if one_published_one_preprint && !different_arxiv && !different_mr && !different_zbl {
        let text_confirms_preprint = text_sim >= 0.78
            && authors_compatible
            && (!(a.meaningful_title && b.meaningful_title) || title_sim >= 0.65);
        if title_author_same_work || text_confirms_preprint {
            return Some(PairMatch {
                level: 4,
                similarity: title_sim.max(text_sim),
                reason: "Versioni diverse (preprint arXiv / pubblicazione)",
            });
        }
    }

    // 3. Level 5: Different editions of the SAME book (requires matching book title + compatible authors!)
    if either_book
        && !different_arxiv
        && (different_isbn || different_edition || (both_books && different_year))
    {
        let same_book_title_and_author = (title_sim >= 0.90 && authors_strong && min_sig_words >= 1)
            || (title_sim >= 0.95 && authors_compatible && min_sig_words >= 3);
        if same_book_title_and_author && !a.is_garbage_title && !b.is_garbage_title {
            return Some(PairMatch {
                level: 5,
                similarity: title_sim.max(text_sim),
                reason: "Possibili edizioni diverse dello stesso libro (anno / edizione / ISBN differenti)",
            });
        }
    }

    // 4. Level 3: Probable Duplicate
    // Any conflicting explicit bibliographic identifier (DOI, MR, Zbl, arXiv, ISBN) rules out a Level 3 duplicate.
    if different_doi || different_mr || different_zbl || different_arxiv || different_isbn {
        return None;
    }

    // Conflicting publication years on books (>= 2 years apart) or distant article years rule out Level 3.
    if let (Some(ya), Some(yb)) = (a.year, b.year) {
        let gap = (ya - yb).abs();
        if (both_books && gap >= 2) || (gap >= 5 && text_sim < 0.90) {
            return None;
        }
    }

    // Path 4A: High Title + Author similarity, guarded against collana/template front-matter headers
    if title_author_same_work && !a.is_garbage_title && !b.is_garbage_title {
        let repeated_heuristic_banner = a.is_heuristic
            && b.is_heuristic
            && (stats.heuristic_title_freq(&a.title_norm) >= 3
                || stats.heuristic_title_freq(&b.title_norm) >= 3);

        let body_text_contradicts =
            a.minhash.is_some() && b.minhash.is_some() && text_sim < 0.22;

        let page_count_ok = page_counts_compatible_for_duplicate(a.page_count, b.page_count);
        let filenames_conflict = a.is_heuristic
            && b.is_heuristic
            && filename_stems_indicate_distinct_works(&a.primary_rel_path, &b.primary_rel_path);

        let passed_guards = if repeated_heuristic_banner || filenames_conflict {
            text_sim >= 0.78
        } else if !page_count_ok {
            text_sim >= 0.82
        } else {
            !(body_text_contradicts && (a.is_heuristic || b.is_heuristic || either_book))
        };

        if passed_guards {
            let au_s = author_sim_opt.unwrap_or(0.85);
            return Some(PairMatch {
                level: 3,
                similarity: (title_sim * 0.7 + au_s * 0.3).max(text_sim),
                reason: "Probabile doppione (titolo/autori o testo estratti molto simili)",
            });
        }
    }

    // Path 4B: High interior body-text MinHash Jaccard similarity
    if text_sim >= 0.82 {
        // Guard against any residual collision when both documents have confirmed, completely distinct metadata
        let metadata_strongly_conflicts = (a.meaningful_title
            && b.meaningful_title
            && title_sim < 0.50
            && (!a.is_heuristic && !b.is_heuristic || author_sim_opt.is_some_and(|s| s < 0.50)))
            || (!page_counts_compatible_for_duplicate(a.page_count, b.page_count)
                && text_sim < 0.92);

        if !metadata_strongly_conflicts {
            return Some(PairMatch {
                level: 3,
                similarity: text_sim,
                reason: "Probabile doppione (testo del corpo documento molto simile)",
            });
        }
    }

    None
}

fn page_counts_compatible_for_duplicate(pc_a: Option<u32>, pc_b: Option<u32>) -> bool {
    let (Some(a), Some(b)) = (pc_a, pc_b) else {
        return true;
    };
    if a == 0 || b == 0 {
        return true;
    }
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let diff = hi - lo;
    if diff <= 10 || (lo * 100 >= hi * 82) {
        return true;
    }
    // Also allow 2-up double-page landscape scans (e.g. 150 double-spreads vs 300 single pages)
    if hi >= 30 && (lo * 2).abs_diff(hi) <= (hi / 12).max(6) {
        return true;
    }
    false
}

fn is_meaningful_prose_title_prechecked(raw: &str, norm: &str, is_garbage: bool) -> bool {
    if raw.trim().len() < 6 || is_garbage {
        return false;
    }
    let alpha_words = norm
        .split_whitespace()
        .filter(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 3)
        .count();
    alpha_words >= 2 || (alpha_words == 1 && norm.len() >= 7 && !raw.contains('_'))
}

fn filename_stems_indicate_distinct_works(rel_a: &str, rel_b: &str) -> bool {
    let stem_a = Path::new(rel_a)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let stem_b = Path::new(rel_b)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let norm_a = normalize_title_words(&stem_a.replace(['_', '-'], " "));
    let norm_b = normalize_title_words(&stem_b.replace(['_', '-'], " "));
    let toks_a = significant_title_tokens(&norm_a);
    let toks_b = significant_title_tokens(&norm_b);
    if toks_a.len() < 2 || toks_b.len() < 2 {
        return false;
    }
    pair_title_sim(&norm_a, &norm_b) < 0.45
}

fn is_edition_or_version_qualifier(s: &str) -> bool {
    let low = normalize_title_words(s);
    const QUALIFIERS: &[&str] = &[
        "edition",
        "edizione",
        "ed",
        "revised",
        "riveduta",
        "ampliata",
        "expanded",
        "corrected",
        "corretta",
        "printing",
        "ristampa",
        "preprint",
        "draft",
        "version",
        "versione",
        "supplement",
    ];
    low.split_whitespace().any(|w| QUALIFIERS.contains(&w))
}

fn strip_known_series_prefix(title: &str, series: Option<&str>) -> String {
    let Some(ser) = series.map(str::trim).filter(|s| s.len() >= 4) else {
        return title.to_string();
    };
    let ser_norm = normalize_title_words(ser);
    if ser_norm.is_empty() {
        return title.to_string();
    }
    for sep in [" - ", " – ", " — ", ": "] {
        if let Some((left, right)) = title.split_once(sep) {
            let left_norm = normalize_title_words(left);
            if (left_norm == ser_norm || left_norm.starts_with(&format!("{ser_norm} ")))
                && right.trim().len() >= 4
            {
                return right.trim().to_string();
            }
        }
    }
    title.to_string()
}

fn compute_dedup_title_similarity(a: &DocSig, b: &DocSig) -> (f32, usize) {
    let ta = &a.stripped_title;
    let tb = &b.stripped_title;

    // Detect "<Shared Series/Collection Prefix> - <Distinct Book Title>" so shared prefixes
    // never inflate Jaro-Winkler similarity across different volumes of the same collana.
    if a.has_sub_sep && b.has_sub_sep {
        for sep in [" - ", " – ", " — ", ": "] {
            if let (Some((pa, sa)), Some((pb, sb))) = (ta.split_once(sep), tb.split_once(sep)) {
                let pa_norm = normalize_title_words(pa);
                let pb_norm = normalize_title_words(pb);
                let sa_norm = normalize_title_words(sa);
                let sb_norm = normalize_title_words(sb);
                if !pa_norm.is_empty()
                    && !sa_norm.is_empty()
                    && !sb_norm.is_empty()
                    && pair_title_sim(&pa_norm, &pb_norm) >= 0.88
                    && !is_edition_or_version_qualifier(sa)
                    && !is_edition_or_version_qualifier(sb)
                {
                    let suffix_sim = pair_title_sim(&sa_norm, &sb_norm);
                    if suffix_sim < 0.55 {
                        return (suffix_sim * 0.5, 1);
                    }
                }
            }
        }
    }

    let (sim_ab, sig_a, _) = compute_title_similarity(ta, tb);
    let (sim_ba, sig_b, _) = compute_title_similarity(tb, ta);
    (sim_ab.min(sim_ba), sig_a.min(sig_b))
}

fn compute_dedup_author_similarity(a_norm: &str, b_norm: &str) -> Option<f32> {
    let a_list: Vec<&str> = a_norm
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let b_list: Vec<&str> = b_norm
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if a_list.is_empty() || b_list.is_empty() {
        return None;
    }

    let score_author_pair = |x: &str, y: &str| -> f32 {
        if x == y {
            return 1.0;
        }
        let x_toks: Vec<&str> = x
            .split_whitespace()
            .filter(|w| w.len() >= 2 && w.chars().any(char::is_alphabetic))
            .collect();
        let y_toks: Vec<&str> = y
            .split_whitespace()
            .filter(|w| w.len() >= 2 && w.chars().any(char::is_alphabetic))
            .collect();
        let jw = jaro_winkler(&x_toks.join(" "), &y_toks.join(" ")) as f32;
        // Compare surnames (last alphabetic token in "given family" normalized summary)
        if let (Some(&xs), Some(&ys)) = (x_toks.last(), y_toks.last()) {
            if xs == ys && xs.len() >= 3 {
                let init_ok = match (x_toks.first(), y_toks.first()) {
                    (Some(&xg), Some(&yg)) if x_toks.len() > 1 && y_toks.len() > 1 => {
                        xg.chars().next() == yg.chars().next()
                    }
                    _ => true,
                };
                if init_ok {
                    return jw.max(0.92);
                }
                return jw.min(0.65);
            }
            let surname_jw = jaro_winkler(xs, ys) as f32;
            if surname_jw < 0.85 {
                return surname_jw * 0.70;
            }
        }
        jw * 0.85
    };

    let (shorter, longer) = if a_list.len() <= b_list.len() {
        (&a_list, &b_list)
    } else {
        (&b_list, &a_list)
    };

    let mut total = 0.0_f32;
    for &au in shorter {
        let best = longer
            .iter()
            .map(|&other| score_author_pair(au, other))
            .fold(0.0_f32, f32::max);
        total += best;
    }
    Some((total / shorter.len() as f32).clamp(0.0, 1.0))
}

fn parse_volume_or_part_token(tok: &str) -> Option<u32> {
    let t = tok.trim_matches(|c: char| !c.is_alphanumeric());
    if t.is_empty() {
        return None;
    }
    if t.chars().all(|c| c.is_ascii_digit()) && t.len() <= 4 {
        let n: u32 = t.parse().ok()?;
        if (1..=999).contains(&n) && !(1850..=2050).contains(&n) {
            return Some(n);
        }
        return None;
    }
    match t {
        "i" | "primo" | "prima" => Some(1),
        "ii" | "secondo" | "seconda" => Some(2),
        "iii" | "terzo" | "terza" => Some(3),
        "iv" | "quarto" | "quarta" => Some(4),
        "v" | "quinto" | "quinta" => Some(5),
        "vi" | "sesto" | "sesta" => Some(6),
        "vii" | "settimo" | "settima" => Some(7),
        "viii" | "ottavo" | "ottava" => Some(8),
        "ix" | "nono" | "nona" => Some(9),
        "x" | "decimo" | "decima" => Some(10),
        "xi" => Some(11),
        "xii" => Some(12),
        "xiii" => Some(13),
        "xiv" => Some(14),
        "xv" => Some(15),
        _ => None,
    }
}

fn normalize_volume_field(vol: Option<&str>) -> Option<String> {
    let raw = vol?.trim();
    if raw.is_empty() {
        return None;
    }
    let low = normalize_title_words(raw);
    for tok in low.split_whitespace() {
        if matches!(tok, "vol" | "volume" | "no" | "nr" | "tomo" | "tome" | "band" | "part") {
            continue;
        }
        if let Some(n) = parse_volume_or_part_token(tok) {
            return Some(n.to_string());
        }
    }
    if low.is_empty() {
        None
    } else {
        Some(low)
    }
}

fn extract_title_volume_or_part(raw_title: &str) -> Option<u32> {
    let norm = normalize_title_words(raw_title);
    let toks: Vec<&str> = norm.split_whitespace().collect();
    if toks.is_empty() {
        return None;
    }

    // Leading index number e.g. "01 Numeri Primi" or "Mondo Matematico 14 ..."
    if toks.len() >= 3 {
        if let Some(n) = toks[0]
            .chars()
            .all(|c| c.is_ascii_digit())
            .then(|| toks[0].parse::<u32>().ok())
            .flatten()
        {
            if (1..=300).contains(&n) {
                return Some(n);
            }
        }
    }

    for i in 0..toks.len() {
        if matches!(
            toks[i],
            "vol"
                | "volume"
                | "part"
                | "parte"
                | "teil"
                | "tome"
                | "tomo"
                | "band"
                | "libro"
                | "book"
                | "fasc"
                | "fascicule"
                | "fascicolo"
                | "heft"
                | "no"
                | "nr"
                | "numero"
                | "num"
                | "quaderno"
                | "dispensa"
                | "lezione"
                | "lecture"
                | "chapter"
                | "capitolo"
        ) {
            if let Some(&next) = toks.get(i + 1) {
                // Make sure next token isn't followed by "edition"/"edizione"
                let after = toks.get(i + 2).copied().unwrap_or("");
                if !matches!(after, "edition" | "edizione" | "ed") {
                    if let Some(n) = parse_volume_or_part_token(next) {
                        return Some(n);
                    }
                }
            }
        }
    }

    // Trailing Roman or integer designator ("Analysis I" vs "Analysis II", "Analisi 1" vs "Analisi 2")
    if toks.len() >= 2 {
        let prev = toks[toks.len() - 2];
        let last = toks[toks.len() - 1];
        if !matches!(
            prev,
            "edition" | "edizione" | "ed" | "in" | "of" | "dim" | "dimension" | "order"
        ) {
            if let Some(n) = parse_volume_or_part_token(last) {
                if n <= 999 && last != "v" {
                    return Some(n);
                }
            }
        }
    }

    None
}

fn has_conflicting_volume_or_part(a: &DocSig, b: &DocSig) -> bool {
    // 1. Explicit `volume` metadata column on books / series (using pre-normalized fields)
    if let (Some(va), Some(vb)) = (&a.norm_volume, &b.norm_volume) {
        if va != vb && (a.work_type == "book" || b.work_type == "book") {
            let same_or_missing_series = match (&a.norm_series, &b.norm_series) {
                (Some(sa), Some(sb)) => sa == sb,
                _ => true,
            };
            if same_or_missing_series {
                return true;
            }
        }
    }

    // 2. Precomputed volume / part designators inside titles
    match (a.title_vol_part, b.title_vol_part) {
        (Some(na), Some(nb)) if na != nb => true,
        // "Analisi Matematica" (implicit Vol. 1) vs "Analisi Matematica 2" / "Part II" (explicit >= 2)
        (None, Some(nb)) if nb >= 2 && pair_title_sim(&a.title_norm, &b.title_norm) >= 0.80 => true,
        (Some(na), None) if na >= 2 && pair_title_sim(&a.title_norm, &b.title_norm) >= 0.80 => true,
        _ => false,
    }
}

pub fn list_duplicate_groups(db: &Database) -> Result<Vec<DuplicateGroupView>> {
    let conn = db.reader();
    let mut g_stmt = conn.prepare(
        "SELECT id, group_level, max_similarity, reason_detail, status
         FROM duplicate_groups
         WHERE status != 'distinct'
         ORDER BY group_level ASC, max_similarity DESC, id DESC",
    )?;
    let groups: Vec<(i64, i64, f32, String, String)> = g_stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, f64>(2)? as f32,
                r.get(3)?,
                r.get(4)?,
            ))
        })?
        .filter_map(std::result::Result::ok)
        .collect();

    if groups.is_empty() {
        return Ok(Vec::new());
    }

    // Batch-load all members for non-'distinct' groups in 1 single SQL query instead of N+1 queries!
    let mut members_by_group: HashMap<i64, Vec<DuplicateFileMember>> =
        HashMap::with_capacity(groups.len());
    {
        let mut m_stmt = conn.prepare(
            "SELECT dgm.group_id, dgm.document_id, f.id, f.root_id, sr.label, f.rel_path, f.format,
                    d.title, d.year, d.edition, d.isbn, d.doi, d.arxiv_id, d.mr_number,
                    d.zbl_number, f.page_count, f.size_bytes, f.is_available
             FROM duplicate_group_members dgm
             JOIN duplicate_groups dg ON dg.id = dgm.group_id
             JOIN files f ON f.id = dgm.file_id
             JOIN documents d ON d.id = dgm.document_id
             JOIN source_roots sr ON sr.id = f.root_id
             WHERE dg.status != 'distinct'
             ORDER BY dgm.group_id ASC, f.id ASC",
        )?;
        let m_rows = m_stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                DuplicateFileMember {
                    document_id: r.get(1)?,
                    file_id: r.get(2)?,
                    root_id: r.get(3)?,
                    root_label: r.get(4)?,
                    rel_path: r.get(5)?,
                    format: r.get(6)?,
                    title: r.get(7)?,
                    year: r.get(8)?,
                    edition: r.get(9)?,
                    isbn: r.get(10)?,
                    doi: r.get(11)?,
                    arxiv_id: r.get(12)?,
                    mr_number: r.get(13)?,
                    zbl_number: r.get(14)?,
                    page_count: r.get::<_, Option<i64>>(15)?.map(|v| v as u32),
                    size_bytes: r.get::<_, i64>(16)? as u64,
                    is_available: r.get::<_, i64>(17)? != 0,
                },
            ))
        })?;
        for r in m_rows {
            let (gid, member) = r?;
            members_by_group.entry(gid).or_default().push(member);
        }
    }

    let mut out = Vec::with_capacity(groups.len());
    for (gid, level, sim, reason, status) in groups {
        if let Some(members) = members_by_group.remove(&gid) {
            if members.len() >= 2 {
                out.push(DuplicateGroupView {
                    group_id: gid,
                    group_level: level,
                    max_similarity: sim,
                    reason_detail: reason,
                    status,
                    members,
                });
            }
        }
    }

    Ok(out)
}

/// Merges duplicate group documents into `keep_document_id`, re-attaching all files/passages
/// without touching any files on disk.
pub fn merge_duplicate_group(db: &Database, group_id: i64, keep_document_id: i64) -> Result<()> {
    let conn = db.writer();
    let now = chrono::Utc::now().timestamp();
    let tx = conn.unchecked_transaction()?;

    let member_docs: Vec<i64> = {
        let mut st = tx.prepare(
            "SELECT DISTINCT document_id FROM duplicate_group_members WHERE group_id = ?1",
        )?;
        let iter = st.query_map(params![group_id], |r| r.get(0))?;
        iter.filter_map(std::result::Result::ok).collect()
    };

    if !member_docs.contains(&keep_document_id) {
        return Err(CoreError::NotFound(format!(
            "document {keep_document_id} not in group {group_id}"
        )));
    }

    for doc_id in member_docs {
        if doc_id == keep_document_id {
            continue;
        }
        tx.execute(
            "UPDATE files SET document_id = ?1 WHERE document_id = ?2",
            params![keep_document_id, doc_id],
        )?;
        tx.execute(
            "UPDATE passages SET document_id = ?1 WHERE document_id = ?2",
            params![keep_document_id, doc_id],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO document_tags (document_id, tag_id)
             SELECT ?1, tag_id FROM document_tags WHERE document_id = ?2",
            params![keep_document_id, doc_id],
        )?;
        tx.execute("DELETE FROM documents WHERE id = ?1", params![doc_id])?;
    }

    tx.execute(
        "UPDATE duplicate_groups SET status = 'merged', resolved_at = ?1 WHERE id = ?2",
        params![now, group_id],
    )?;
    // Clean up any other pending groups that dropped below 2 distinct documents after the merge
    tx.execute(
        "DELETE FROM duplicate_groups
         WHERE group_level >= 2
           AND status = 'pending'
           AND id NOT IN (
               SELECT group_id FROM duplicate_group_members
               GROUP BY group_id HAVING COUNT(DISTINCT document_id) >= 2
           )",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

/// Marks a duplicate group with a user decision: `'distinct'`, `'versions'`, or `'editions'`.
pub fn resolve_duplicate_group(db: &Database, group_id: i64, decision: &str) -> Result<()> {
    if !matches!(decision, "distinct" | "versions" | "editions") {
        return Err(CoreError::Config(format!(
            "invalid duplicate decision: {decision}"
        )));
    }
    let conn = db.writer();
    let now = chrono::Utc::now().timestamp();

    let sig_map: HashMap<i64, String> = load_doc_signatures(&conn)?
        .into_iter()
        .map(|d| (d.id, d.pair_sig_hash))
        .collect();

    let tx = conn.unchecked_transaction()?;

    tx.execute(
        "UPDATE duplicate_groups SET status = ?1, resolved_at = ?2 WHERE id = ?3",
        params![decision, now, group_id],
    )?;

    let doc_ids: Vec<i64> = {
        let mut st = tx.prepare(
            "SELECT DISTINCT document_id FROM duplicate_group_members WHERE group_id = ?1 ORDER BY document_id ASC",
        )?;
        let iter = st.query_map(params![group_id], |r| r.get(0))?;
        iter.filter_map(std::result::Result::ok).collect()
    };

    for i in 0..doc_ids.len() {
        for j in (i + 1)..doc_ids.len() {
            let h_min = sig_map
                .get(&doc_ids[i])
                .map(String::as_str)
                .unwrap_or("");
            let h_max = sig_map
                .get(&doc_ids[j])
                .map(String::as_str)
                .unwrap_or("");
            tx.execute(
                "INSERT INTO duplicate_evaluated_pairs (doc_id_min, doc_id_max, hash_sig_min, hash_sig_max, decision, evaluated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(doc_id_min, doc_id_max) DO UPDATE SET
                    decision = excluded.decision,
                    evaluated_at = excluded.evaluated_at",
                params![doc_ids[i], doc_ids[j], h_min, h_max, decision, now],
            )?;
        }
    }

    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::docs::{create_heuristic_document, insert_file_record, NewFileRecord};
    use crate::db::repos::roots::{insert_source_root, NewSourceRoot};
    use crate::dedup::{compute_document_minhash_from_pages, compute_minhash, pack_minhash};
    use tempfile::tempdir;

    #[test]
    fn incremental_dedup_remembers_distinct_decision_until_changed() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("papero.db"), 2).unwrap();
        let root_id = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Lib".into(),
                    root_path: dir.path().to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap()
            .id
        };

        let (d1, d2, d3) = {
            let w = db.writer();
            let d1 = create_heuristic_document(&w, "Modular elliptic curves").unwrap();
            let d2 = create_heuristic_document(&w, "Modular elliptic curves preprint").unwrap();
            let d3 = create_heuristic_document(&w, "Differential Topology").unwrap();

            w.execute(
                "UPDATE documents SET doi = '10.2307/2118559', mr_number = '1333035' WHERE id = ?1",
                params![d1],
            )
            .unwrap();
            w.execute(
                "UPDATE documents SET arxiv_id = 'alg-geom/9501001', doi = '10.2307/2118559' WHERE id = ?1",
                params![d2],
            )
            .unwrap();

            insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: d1,
                    root_id: &root_id,
                    rel_path: "a.pdf",
                    format: "pdf",
                    size_bytes: 10,
                    mtime_ns: 1,
                    blake3_hash: &"a".repeat(64),
                    index_state: "ready",
                },
            )
            .unwrap();
            insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: d2,
                    root_id: &root_id,
                    rel_path: "b.pdf",
                    format: "pdf",
                    size_bytes: 12,
                    mtime_ns: 2,
                    blake3_hash: &"b".repeat(64),
                    index_state: "ready",
                },
            )
            .unwrap();
            insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: d3,
                    root_id: &root_id,
                    rel_path: "c.pdf",
                    format: "pdf",
                    size_bytes: 14,
                    mtime_ns: 3,
                    blake3_hash: &"c".repeat(64),
                    index_state: "ready",
                },
            )
            .unwrap();
            (d1, d2, d3)
        };
        let _ = (d1, d2, d3);

        let r1 = run_duplicate_detection(&db).unwrap();
        assert_eq!(r1.new_groups_created, 1);

        let groups = list_duplicate_groups(&db).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group_level, 4); // arXiv preprint + published DOI -> versions!

        resolve_duplicate_group(&db, groups[0].group_id, "distinct").unwrap();
        assert!(list_duplicate_groups(&db).unwrap().is_empty());

        // Second run skips cached pairs and does NOT re-suggest dismissed pair
        let r2 = run_duplicate_detection(&db).unwrap();
        assert_eq!(r2.new_groups_created, 0);
        assert_eq!(r2.pairs_skipped_cached, 3);
    }

    #[test]
    fn books_in_same_collana_with_identical_initial_pages_are_not_duplicates() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("papero.db"), 2).unwrap();
        let root_id = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Books".into(),
                    root_path: dir.path().to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap()
            .id
        };

        // Shared 18-page initial collana presentation & catalog across 42 distinct books
        let shared_front_page = "Il Mondo Matematico - Collana diretta da Comitato Scientifico Internazionale. \
            Presentazione della collana: la matematica moderna attraverso i grandi teoremi, \
            dalla teoria dei numeri alla geometria differenziale e alla topologia algebrica. \
            Volumi pubblicati nella stessa collana: 1. I numeri primi; 2. La sezione aurea; \
            3. Crittografia e codici segreti; 4. Il teorema di Fermat; 5. La congettura di Poincare.";

        let topics = [
            ("Enrique Gracian", "I numeri primi", "prime zeta riemann euler distribution"),
            ("Fernando Corbalan", "La sezione aurea", "fibonacci golden ratio pentagon spiral"),
            ("Joan Gomez", "Quando le rette diventano curve", "lobachevsky hyperbolic non euclidean gauss"),
            (" Claudi Alsina", "La setta dei numeri", "pythagoras harmonic ratios figurate numbers"),
            ("Javier Fresan", "Il sogno della ragione", "godel incompleteness formalhilbert logic"),
            (" Gustavo Ernesto Pineiro", "La sfera che volle essere quadro", "tarski banach measure paradox"),
        ];

        {
            let w = db.writer();
            for i in 0..42usize {
                let (author, base_title, vocab) = topics[i % topics.len()];
                let distinct_title = format!("Il Mondo Matematico - {base_title} (studio {})", i + 1);
                let doc_id = create_heuristic_document(&w, &distinct_title).unwrap();
                let mut pages: Vec<(u32, String)> = Vec::new();
                for p in 1..=18u32 {
                    pages.push((p, shared_front_page.to_string()));
                }
                for p in 19..=140u32 {
                    pages.push((
                        p,
                        format!(
                            "Capitolo {} del volume {} dedicato a {vocab}. Dimostriamo il risultato \
                             principale per l'indice {} mediante costruzioni esplicite uniche del testo {} \
                             con argomenti specifici {vocab} pagina {} sequenza {}.",
                            (p - 18) / 10 + 1,
                            i + 1,
                            i * 97 + p as usize,
                            base_title,
                            p,
                            i * 1009 + p as usize * 31
                        ),
                    ));
                }
                let mh = compute_document_minhash_from_pages(
                    pages.iter().map(|(ord, s)| (*ord, s.as_str())),
                    140,
                )
                .expect("body minhash");
                w.execute(
                    "UPDATE documents SET work_type = 'book', authors_norm_summary = ?1, minhash_sig = ?2 WHERE id = ?3",
                    params![author.trim().to_lowercase(), pack_minhash(&mh), doc_id],
                )
                .unwrap();

                let fid = insert_file_record(
                    &w,
                    &NewFileRecord {
                        document_id: doc_id,
                        root_id: &root_id,
                        rel_path: &format!("collana/vol_{:02}_{}.pdf", i + 1, base_title.replace(' ', "_")),
                        format: "pdf",
                        size_bytes: 5_000_000 + (i as u64) * 10_000,
                        mtime_ns: 100 + i as i64,
                        blake3_hash: &format!("{:064x}", i + 1),
                        index_state: "ready",
                    },
                )
                .unwrap();
                w.execute(
                    "UPDATE files SET page_count = ?1 WHERE id = ?2",
                    params![140 + (i % 15) as i64, fid],
                )
                .unwrap();
            }
        }

        // Previously C(42, 2) = 861 false duplicate groups were created; now 0!
        let report = run_duplicate_detection(&db).unwrap();
        assert_eq!(report.new_groups_created, 0);
        assert!(list_duplicate_groups(&db).unwrap().is_empty());
    }

    #[test]
    fn multi_copy_duplicates_cluster_into_single_group_and_stale_pending_prunes() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("papero.db"), 2).unwrap();
        let root_id = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Lib".into(),
                    root_path: dir.path().to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap()
            .id
        };

        let shared_body = "Let M be a complete Riemannian manifold with nonnegative Ricci curvature \
            satisfying the Li-Yau parabolic Harnack inequality and Bishop-Gromov volume comparison \
            theorem across geodesic balls of arbitrary radius on M.";
        let mh_blob = pack_minhash(&compute_minhash(shared_body).unwrap());

        // 4 copies of the exact same paper -> should form 1 single group of 4 members, NOT C(4,2) = 6 groups!
        let mut copy_ids = Vec::new();
        {
            let w = db.writer();
            for idx in 0..4usize {
                let d = create_heuristic_document(
                    &w,
                    "Li-Yau Harnack Inequalities on Riemannian Manifolds",
                )
                .unwrap();
                w.execute(
                    "UPDATE documents SET authors_norm_summary = 'peter li; shing tung yau', minhash_sig = ?1 WHERE id = ?2",
                    params![mh_blob, d],
                )
                .unwrap();
                let fid = insert_file_record(
                    &w,
                    &NewFileRecord {
                        document_id: d,
                        root_id: &root_id,
                        rel_path: &format!("copy_{idx}.pdf"),
                        format: "pdf",
                        size_bytes: 1000 + idx as u64,
                        mtime_ns: 10 + idx as i64,
                        blake3_hash: &format!("{:064x}", 500 + idx),
                        index_state: "ready",
                    },
                )
                .unwrap();
                w.execute("UPDATE files SET page_count = 28 WHERE id = ?1", params![fid])
                    .unwrap();
                copy_ids.push(d);
            }
        }

        let r1 = run_duplicate_detection(&db).unwrap();
        assert_eq!(r1.new_groups_created, 1);
        let groups = list_duplicate_groups(&db).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 4);

        // Now suppose copy_ids[3] turns out to be Part II with a different DOI & title after metadata resolution:
        // re-running dedup must prune copy_ids[3] and keep a clean 3-member group!
        {
            let w = db.writer();
            w.execute(
                "UPDATE documents SET title = 'Unrelated Geometric Flows', title_norm = 'unrelated geometric flows',
                 authors_norm_summary = 'richard hamilton', doi = '10.4310/jdg/999999', metadata_source = 'crossref',
                 minhash_sig = NULL WHERE id = ?1",
                params![copy_ids[3]],
            )
            .unwrap();
        }

        let _ = run_duplicate_detection(&db).unwrap();
        let groups_after = list_duplicate_groups(&db).unwrap();
        assert_eq!(groups_after.len(), 1);
        assert_eq!(groups_after[0].members.len(), 3);
    }

    #[test]
    fn multi_volume_works_and_same_author_series_stay_distinct_while_editions_match() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("papero.db"), 2).unwrap();
        let root_id = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Lib".into(),
                    root_path: dir.path().to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap()
            .id
        };

        {
            let w = db.writer();
            // Vol. I vs Vol. II of the same multi-volume textbook -> MUST NOT match
            let v1 = create_heuristic_document(&w, "Methods of Modern Mathematical Physics I").unwrap();
            let v2 = create_heuristic_document(&w, "Methods of Modern Mathematical Physics II").unwrap();
            // Serge Lang: two different books in Graduate Texts in Mathematics -> MUST NOT match
            let lang_alg1 = create_heuristic_document(&w, "Algebra").unwrap();
            let lang_alg3 = create_heuristic_document(&w, "Algebra").unwrap();
            let lang_ca = create_heuristic_document(&w, "Complex Analysis").unwrap();

            for (idx, (doc_id, au, wt, ser, vol, yr, ed, isbn, pc)) in [
                (
                    v1,
                    "michael reed; barry simon",
                    "book",
                    None,
                    Some("1"),
                    Some(1972i64),
                    None,
                    Some("9780125850018"),
                    325i64,
                ),
                (
                    v2,
                    "michael reed; barry simon",
                    "book",
                    None,
                    Some("2"),
                    Some(1975),
                    None,
                    Some("9780125850025"),
                    361,
                ),
                (
                    lang_alg1,
                    "serge lang",
                    "book",
                    Some("Graduate Texts in Mathematics"),
                    Some("211"),
                    Some(1984),
                    Some("2"),
                    Some("9780201041774"),
                    714,
                ),
                (
                    lang_alg3,
                    "serge lang",
                    "book",
                    Some("Graduate Texts in Mathematics"),
                    Some("211"),
                    Some(2002),
                    Some("3"),
                    Some("9780387953854"),
                    914,
                ),
                (
                    lang_ca,
                    "serge lang",
                    "book",
                    Some("Graduate Texts in Mathematics"),
                    Some("103"),
                    Some(1999),
                    Some("4"),
                    Some("9780387985923"),
                    485,
                ),
            ]
            .into_iter()
            .enumerate()
            {
                w.execute(
                    "UPDATE documents SET authors_norm_summary = ?1, work_type = ?2, series = ?3,
                     volume = ?4, year = ?5, edition = ?6, isbn = ?7, metadata_source = 'zbmath'
                     WHERE id = ?8",
                    params![au, wt, ser, vol, yr, ed, isbn, doc_id],
                )
                .unwrap();
                let fid = insert_file_record(
                    &w,
                    &NewFileRecord {
                        document_id: doc_id,
                        root_id: &root_id,
                        rel_path: &format!("b_{idx}.pdf"),
                        format: "pdf",
                        size_bytes: 2_000_000 + idx as u64,
                        mtime_ns: 50 + idx as i64,
                        blake3_hash: &format!("{:064x}", 900 + idx),
                        index_state: "ready",
                    },
                )
                .unwrap();
                w.execute("UPDATE files SET page_count = ?1 WHERE id = ?2", params![pc, fid])
                    .unwrap();
            }
        }

        let r = run_duplicate_detection(&db).unwrap();
        assert_eq!(r.new_groups_created, 1);
        let groups = list_duplicate_groups(&db).unwrap();
        assert_eq!(groups.len(), 1);
        // Only Serge Lang's Algebra 2nd ed. vs 3rd ed. matches, as Level 5 (book_editions)!
        assert_eq!(groups[0].group_level, 5);
        assert_eq!(groups[0].members.len(), 2);
        assert!(groups[0].members.iter().all(|m| m.title == "Algebra"));
    }

    #[test]
    fn migration_v6_purges_820_legacy_collana_duplicates_and_keeps_true_matches() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("legacy_v5.db");
        let db = Database::open(&db_path, 2).unwrap();
        let root_id = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Collana".into(),
                    root_path: dir.path().to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap()
            .id
        };

        // Simulate legacy v5 DB state: 41 books in a collana had identical front-matter minhash_sig
        // and all C(41, 2) = 820 pairs were stored as pending Level 3 duplicate_groups!
        let legacy_front_sig = pack_minhash(
            &compute_minhash(
                "Collana Mondo Matematico presentazione generale della collana scientifica \
                 con elenco completo di tutti i quaranta volumi pubblicati nella serie editoriale.",
            )
            .unwrap(),
        );

        let mut collana_docs = Vec::new();
        {
            let w = db.writer();
            for i in 0..41usize {
                let d = create_heuristic_document(&w, &format!("Volume tematico autonomo numero {}", i + 1)).unwrap();
                w.execute(
                    "UPDATE documents SET work_type = 'book', authors_norm_summary = ?1, minhash_sig = ?2 WHERE id = ?3",
                    params![format!("autore distinto {}", i + 1), legacy_front_sig, d],
                )
                .unwrap();
                let fid = insert_file_record(
                    &w,
                    &NewFileRecord {
                        document_id: d,
                        root_id: &root_id,
                        rel_path: &format!("c_{i}.pdf"),
                        format: "pdf",
                        size_bytes: 3_000_000 + i as u64,
                        mtime_ns: 1 + i as i64,
                        blake3_hash: &format!("{:064x}", 2000 + i),
                        index_state: "ready",
                    },
                )
                .unwrap();
                w.execute("UPDATE files SET page_count = 160 WHERE id = ?1", params![fid])
                    .unwrap();
                // Insert real distinct interior passages (pages 35..=42)
                for pg in 35..=42i64 {
                    w.execute(
                        "INSERT INTO passages (document_id, file_id, page_ord, chunk_index, is_alt_formula, token_count, text_content, bboxes_blob)
                         VALUES (?1, ?2, ?3, 0, 0, 60, ?4, X'')",
                        params![
                            d,
                            fid,
                            pg,
                            format!(
                                "Contenuto matematico esclusivo libro {} pagina {} su argomento specifico {} \
                                 con dimostrazione dettagliata indipendente codice {} risultato {} teorema {} \
                                 lemma {} corollario {} struttura {} varieta {} gruppo {} anello {} modulo {}.",
                                i + 1,
                                pg,
                                i * 313 + pg as usize,
                                i * 997,
                                i * 41 + 7,
                                i * 53 + pg as usize,
                                i * 71 + 3,
                                i * 89 + 11,
                                i * 101 + pg as usize,
                                i * 131,
                                i * 151,
                                i * 173,
                                i * 197
                            )
                        ],
                    )
                    .unwrap();
                }
                collana_docs.push((d, fid));
            }

            // Also add 1 genuine duplicate pair (shared DOI)
            let real_a = create_heuristic_document(&w, " Hodge Theory on Kahler Manifolds").unwrap();
            let real_b = create_heuristic_document(&w, "Hodge Theory on Kahler Manifolds (copy)").unwrap();
            for (k, rd) in [real_a, real_b].into_iter().enumerate() {
                w.execute(
                    "UPDATE documents SET doi = '10.1007/s00222-020-00999-1' WHERE id = ?1",
                    params![rd],
                )
                .unwrap();
                insert_file_record(
                    &w,
                    &NewFileRecord {
                        document_id: rd,
                        root_id: &root_id,
                        rel_path: &format!("real_{k}.pdf"),
                        format: "pdf",
                        size_bytes: 500_000,
                        mtime_ns: 900 + k as i64,
                        blake3_hash: &format!("{:064x}", 8000 + k),
                        index_state: "ready",
                    },
                )
                .unwrap();
            }

            // Populate all C(41, 2) = 820 false pending groups (except mark pair (0,1) as user-resolved 'distinct')
            let tx = w.unchecked_transaction().unwrap();
            for i in 0..collana_docs.len() {
                for j in (i + 1)..collana_docs.len() {
                    let status = if i == 0 && j == 1 { "distinct" } else { "pending" };
                    tx.execute(
                        "INSERT INTO duplicate_groups (group_level, max_similarity, reason_detail, status, created_at)
                         VALUES (3, 0.96, 'Legacy front-matter match', ?1, 1000)",
                        params![status],
                    )
                    .unwrap();
                    let gid = tx.last_insert_rowid();
                    tx.execute(
                        "INSERT INTO duplicate_group_members (group_id, document_id, file_id)
                         VALUES (?1, ?2, ?3), (?1, ?4, ?5)",
                        params![gid, collana_docs[i].0, collana_docs[i].1, collana_docs[j].0, collana_docs[j].1],
                    )
                    .unwrap();
                    tx.execute(
                        "INSERT INTO duplicate_evaluated_pairs (doc_id_min, doc_id_max, hash_sig_min, hash_sig_max, decision, evaluated_at)
                         VALUES (?1, ?2, 'old_a', 'old_b', ?3, 1000)",
                        params![collana_docs[i].0, collana_docs[j].0, status],
                    )
                    .unwrap();
                }
            }
            // Rewind schema_version to 5 so reopening runs Migration v6!
            tx.execute("DELETE FROM schema_version WHERE version >= 6", [])
                .unwrap();
            tx.commit().unwrap();
        }
        drop(db);

        // Reopen database -> Migration v6 runs automatically!
        let upgraded_db = Database::open(&db_path, 2).unwrap();
        let groups = list_duplicate_groups(&upgraded_db).unwrap();
        // All 819 false pending collana groups are gone; ONLY the 1 genuine DOI duplicate remains!
        assert_eq!(groups.len(), 1, "expected only 1 true duplicate group, got {}", groups.len());
        assert_eq!(groups[0].group_level, 2);

        // And the user's explicit 'distinct' decision on (collana_docs[0], collana_docs[1]) was preserved!
        let preserved_distinct: String = upgraded_db
            .reader()
            .query_row(
                "SELECT decision FROM duplicate_evaluated_pairs WHERE doc_id_min = ?1 AND doc_id_max = ?2",
                params![collana_docs[0].0, collana_docs[1].0],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(preserved_distinct, "distinct");
    }

    #[test]
    fn dedup_2000_docs_cold_and_incremental_latency_stays_lightweight() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("perf.db"), 2).unwrap();
        let root_id = {
            let w = db.writer();
            insert_source_root(
                &w,
                &NewSourceRoot {
                    id: "r1".into(),
                    label: "Perf".into(),
                    root_path: dir.path().to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap()
            .id
        };

        let mut last_doc = 0i64;
        {
            let w = db.writer();
            let tx = w.unchecked_transaction().unwrap();
            for i in 0..2000usize {
                // 80 distinct topic buckets of 25 docs each -> ~24,000 candidate pairs!
                let title = format!(
                    "Topicbucket{:03} Subtopic{:03} Analysis Number {}",
                    i % 80,
                    (i / 80) * 13 + (i % 7),
                    i + 10
                );
                let d = create_heuristic_document(&tx, &title).unwrap();
                let mut fake_mh = [0u64; MINHASH_LEN];
                for (slot, val) in fake_mh.iter_mut().enumerate() {
                    *val = (i as u64)
                        .wrapping_mul(0x9e3779b97f4a7c15)
                        .wrapping_add(slot as u64);
                }
                tx.execute(
                    "UPDATE documents SET authors_norm_summary = ?1, year = ?2, minhash_sig = ?3 WHERE id = ?4",
                    params![
                        format!("author_{} surname_{}", i % 97, i % 311),
                        1980 + (i % 45) as i64,
                        pack_minhash(&fake_mh),
                        d
                    ],
                )
                .unwrap();
                insert_file_record(
                    &tx,
                    &NewFileRecord {
                        document_id: d,
                        root_id: &root_id,
                        rel_path: &format!("doc_{i}.pdf"),
                        format: "pdf",
                        size_bytes: 200_000 + i as u64,
                        mtime_ns: 1 + i as i64,
                        blake3_hash: &format!("{:064x}", 50_000 + i),
                        index_state: "ready",
                    },
                )
                .unwrap();
                last_doc = d;
            }
            tx.commit().unwrap();
        }

        let t0 = std::time::Instant::now();
        let cold = run_duplicate_detection(&db).unwrap();
        let cold_ms = t0.elapsed().as_secs_f64() * 1000.0;

        // Simulate editing 1 document's metadata (incremental re-run)
        {
            let w = db.writer();
            w.execute(
                "UPDATE documents SET year = 2024 WHERE id = ?1",
                params![last_doc],
            )
            .unwrap();
        }
        let t1 = std::time::Instant::now();
        let warm = run_duplicate_detection(&db).unwrap();
        let warm_ms = t1.elapsed().as_secs_f64() * 1000.0;

        println!(
            "2000-doc dedup benchmark (debug profile): cold={cold_ms:.1}ms (checked={}), incremental={warm_ms:.1}ms (checked={}, cached={})",
            cold.pairs_checked, warm.pairs_checked, warm.pairs_skipped_cached
        );
        // Even in unoptimized debug profile (opt-level=0), 34k pairs runs < 600ms (< 25ms in release)!
        assert!(cold_ms < 800.0, "cold dedup too slow: {cold_ms:.1}ms");
        assert!(warm_ms < 450.0, "incremental dedup too slow: {warm_ms:.1}ms");
    }
}
