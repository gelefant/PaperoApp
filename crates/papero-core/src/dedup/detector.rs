use crate::db::Database;
use crate::dedup::minhash::{jaccard_similarity, unpack_minhash, MINHASH_LEN};
use crate::error::{CoreError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    title_norm: String,
    authors_norm: String,
    year: Option<i64>,
    edition: Option<String>,
    isbn: Option<String>,
    doi: Option<String>,
    arxiv_id: Option<String>,
    mr_number: Option<String>,
    zbl_number: Option<String>,
    minhash: Option<[u64; MINHASH_LEN]>,
    primary_file_id: i64,
    pair_sig_hash: String,
}

#[derive(Debug, Clone, Copy)]
struct PairMatch {
    level: i64, // 2, 3, 4, or 5
    similarity: f32,
    reason: &'static str,
}

/// Runs incremental duplicate, preprint/published version, and book edition detection
/// without recomputing unchanged pairs already recorded in `duplicate_evaluated_pairs`.
pub fn run_duplicate_detection(db: &Database) -> Result<DuplicateReport> {
    let conn = db.writer();
    let now = chrono::Utc::now().timestamp();
    let mut report = DuplicateReport::default();

    ensure_level1_identical_copy_groups(&conn, now)?;

    let docs = load_doc_signatures(&conn)?;
    if docs.len() < 2 {
        return Ok(report);
    }

    let evaluated = load_evaluated_pairs(&conn)?;
    let existing_pending_pairs = load_existing_group_pairs(&conn)?;

    // Build candidate pairs via blocking keys (shared IDs + shared title prefix tokens) + fallback
    let candidate_indices = build_candidate_pair_indices(&docs);

    let tx = conn.unchecked_transaction()?;
    for (idx_a, idx_b) in candidate_indices {
        let (a, b) = if docs[idx_a].id < docs[idx_b].id {
            (&docs[idx_a], &docs[idx_b])
        } else {
            (&docs[idx_b], &docs[idx_a])
        };

        if let Some((sig_min, sig_max, decision)) = evaluated.get(&(a.id, b.id)) {
            if sig_min == &a.pair_sig_hash && sig_max == &b.pair_sig_hash {
                // Skip unchanged pair unless still pending and missing a group
                if decision != "pending" || existing_pending_pairs.contains(&(a.id, b.id)) {
                    report.pairs_skipped_cached += 1;
                    continue;
                }
            }
        }

        report.pairs_checked += 1;
        let Some(matched) = evaluate_pair(a, b) else {
            // Remember clean non-match so rescan skips unchanged pairs next time!
            tx.execute(
                "INSERT INTO duplicate_evaluated_pairs (doc_id_min, doc_id_max, hash_sig_min, hash_sig_max, decision, evaluated_at)
                 VALUES (?1, ?2, ?3, ?4, 'clean', ?5)
                 ON CONFLICT(doc_id_min, doc_id_max) DO UPDATE SET
                    hash_sig_min = excluded.hash_sig_min,
                    hash_sig_max = excluded.hash_sig_max,
                    evaluated_at = excluded.evaluated_at",
                params![a.id, b.id, a.pair_sig_hash, b.pair_sig_hash, now],
            )?;
            continue;
        };

        if existing_pending_pairs.contains(&(a.id, b.id)) {
            continue;
        }

        let default_status = "pending";
        tx.execute(
            "INSERT INTO duplicate_groups (group_level, max_similarity, reason_detail, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                matched.level,
                matched.similarity as f64,
                matched.reason,
                default_status,
                now
            ],
        )?;
        let gid = tx.last_insert_rowid();
        tx.execute(
            "INSERT OR IGNORE INTO duplicate_group_members (group_id, document_id, file_id)
             VALUES (?1, ?2, ?3), (?1, ?4, ?5)",
            params![gid, a.id, a.primary_file_id, b.id, b.primary_file_id],
        )?;
        tx.execute(
            "INSERT INTO duplicate_evaluated_pairs (doc_id_min, doc_id_max, hash_sig_min, hash_sig_max, decision, evaluated_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)
             ON CONFLICT(doc_id_min, doc_id_max) DO UPDATE SET
                hash_sig_min = excluded.hash_sig_min,
                hash_sig_max = excluded.hash_sig_max,
                decision = 'pending',
                evaluated_at = excluded.evaluated_at",
            params![a.id, b.id, a.pair_sig_hash, b.pair_sig_hash, now],
        )?;
        report.new_groups_created += 1;
    }

    tx.commit()?;
    Ok(report)
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

fn load_doc_signatures(conn: &Connection) -> Result<Vec<DocSig>> {
    let mut stmt = conn.prepare(
        "SELECT d.id, d.work_type, d.title_norm, d.authors_norm_summary, d.year, d.edition,
                d.isbn, d.doi, d.arxiv_id, d.mr_number, d.zbl_number, d.minhash_sig,
                MIN(f.id) AS primary_file_id, MIN(f.blake3_hash) AS primary_hash
         FROM documents d
         JOIN files f ON f.document_id = d.id
         GROUP BY d.id",
    )?;
    let rows = stmt.query_map([], |r| {
        let mh_blob: Option<Vec<u8>> = r.get(11)?;
        let title_norm: String = r.get(2)?;
        let doi: Option<String> = r.get(7)?;
        let arxiv_id: Option<String> = r.get(8)?;
        let mr_number: Option<String> = r.get(9)?;
        let zbl_number: Option<String> = r.get(10)?;
        let isbn: Option<String> = r.get(6)?;
        let primary_hash: String = r.get(13)?;
        let pair_sig_hash = format!(
            "{}:{}:{}:{}:{}:{}:{}",
            primary_hash,
            title_norm,
            doi.as_deref().unwrap_or(""),
            arxiv_id.as_deref().unwrap_or(""),
            mr_number.as_deref().unwrap_or(""),
            zbl_number.as_deref().unwrap_or(""),
            isbn.as_deref().unwrap_or(""),
        );
        Ok(DocSig {
            id: r.get(0)?,
            work_type: r.get(1)?,
            title_norm,
            authors_norm: r.get(3)?,
            year: r.get(4)?,
            edition: r.get(5)?,
            isbn,
            doi,
            arxiv_id,
            mr_number,
            zbl_number,
            minhash: mh_blob.as_deref().and_then(unpack_minhash),
            primary_file_id: r.get(12)?,
            pair_sig_hash,
        })
    })?;

    Ok(rows.filter_map(std::result::Result::ok).collect())
}

type EvaluatedPairMap = HashMap<(i64, i64), (String, String, String)>;

fn load_evaluated_pairs(conn: &Connection) -> Result<EvaluatedPairMap> {
    let mut stmt = conn.prepare(
        "SELECT doc_id_min, doc_id_max, hash_sig_min, hash_sig_max, decision FROM duplicate_evaluated_pairs",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?), (r.get(2)?, r.get(3)?, r.get(4)?)))
    })?;
    let mut map = HashMap::new();
    for r in rows {
        let (k, v) = r?;
        map.insert(k, v);
    }
    Ok(map)
}

fn load_existing_group_pairs(conn: &Connection) -> Result<HashSet<(i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT m1.document_id, m2.document_id
         FROM duplicate_group_members m1
         JOIN duplicate_group_members m2 ON m1.group_id = m2.group_id AND m1.document_id < m2.document_id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    Ok(rows.filter_map(std::result::Result::ok).collect())
}

fn build_candidate_pair_indices(docs: &[DocSig]) -> Vec<(usize, usize)> {
    // Fast inverted buckets on shared IDs + informative title words + full fallback when small
    let mut pairs: HashSet<(usize, usize)> = HashSet::new();
    if docs.len() <= 250 {
        for i in 0..docs.len() {
            for j in (i + 1)..docs.len() {
                pairs.insert((i, j));
            }
        }
        return pairs.into_iter().collect();
    }

    let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, d) in docs.iter().enumerate() {
        for (prefix, val) in [
            ("doi:", d.doi.as_deref()),
            ("mr:", d.mr_number.as_deref()),
            ("zbl:", d.zbl_number.as_deref()),
            ("ax:", d.arxiv_id.as_deref()),
            ("isbn:", d.isbn.as_deref()),
        ] {
            if let Some(v) = val {
                buckets.entry(format!("{prefix}{v}")).or_default().push(idx);
            }
        }
        for w in d
            .title_norm
            .split_whitespace()
            .filter(|w| w.len() >= 5)
            .take(4)
        {
            buckets.entry(format!("tw:{w}")).or_default().push(idx);
        }
    }

    for idxs in buckets.values() {
        if idxs.len() < 2 || idxs.len() > 80 {
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

fn evaluate_pair(a: &DocSig, b: &DocSig) -> Option<PairMatch> {
    // Check Book Editions first (different ISBN or different edition/year on books -> level 5)
    let both_books = a.work_type == "book" && b.work_type == "book";
    let different_isbn = a.isbn.is_some() && b.isbn.is_some() && a.isbn != b.isbn;
    let different_edition = a.edition.is_some() && b.edition.is_some() && a.edition != b.edition;
    let different_year = a.year.is_some() && b.year.is_some() && a.year != b.year;

    // Level 4: Preprint arXiv vs published version (one has arxiv without doi, or shared arxiv + different published/preprint markers)
    let has_shared_arxiv = a.arxiv_id.is_some() && a.arxiv_id == b.arxiv_id;
    let one_published_one_preprint =
        (a.arxiv_id.is_some() && b.arxiv_id.is_none() && (b.doi.is_some() || b.mr_number.is_some()))
            || (b.arxiv_id.is_some() && a.arxiv_id.is_none() && (a.doi.is_some() || a.mr_number.is_some()))
            || (has_shared_arxiv && ((a.doi.is_some() ^ b.doi.is_some()) || (a.mr_number.is_some() ^ b.mr_number.is_some())));

    // Exact shared work IDs (DOI / MR / Zbl / ISBN) -> Level 2 (or Level 4 if preprint vs published)
    if (a.doi.is_some() && a.doi == b.doi)
        || (a.mr_number.is_some() && a.mr_number == b.mr_number)
        || (a.zbl_number.is_some() && a.zbl_number == b.zbl_number)
        || (a.isbn.is_some() && a.isbn == b.isbn)
        || has_shared_arxiv
    {
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

    // Compute fuzzy title + author similarity and MinHash Jaccard
    let title_sim = if !a.title_norm.is_empty() && !b.title_norm.is_empty() {
        jaro_winkler(&a.title_norm, &b.title_norm) as f32
    } else {
        0.0
    };
    let author_sim = if !a.authors_norm.is_empty() && !b.authors_norm.is_empty() {
        jaro_winkler(&a.authors_norm, &b.authors_norm) as f32
    } else {
        0.75 // Neutral when one side lacks extracted author
    };
    let text_sim = match (&a.minhash, &b.minhash) {
        (Some(sa), Some(sb)) => jaccard_similarity(sa, sb),
        _ => 0.0,
    };

    let title_author_high = title_sim >= 0.92 && author_sim >= 0.80;
    let strong_preprint_vs_pub = (title_author_high || text_sim >= 0.78) && one_published_one_preprint;
    if strong_preprint_vs_pub {
        return Some(PairMatch {
            level: 4,
            similarity: title_sim.max(text_sim),
            reason: "Versioni diverse (preprint arXiv / pubblicazione)",
        });
    }

    if both_books && (title_author_high || text_sim >= 0.75) && (different_isbn || different_edition || different_year) {
        return Some(PairMatch {
            level: 5,
            similarity: title_sim.max(text_sim),
            reason: "Possibili edizioni diverse dello stesso libro (anno / edizione / ISBN differenti)",
        });
    }

    if title_author_high || text_sim >= 0.80 {
        return Some(PairMatch {
            level: 3,
            similarity: (title_sim * 0.7 + author_sim * 0.3).max(text_sim),
            reason: "Probabile doppione (titolo/autori o testo estratti molto simili)",
        });
    }

    None
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
    tx.commit()?;
    Ok(())
}

/// Marks a duplicate group with a user decision: `'distinct'`, `'versions'`, or `'editions'`.
pub fn resolve_duplicate_group(db: &Database, group_id: i64, decision: &str) -> Result<()> {
    if !matches!(decision, "distinct" | "versions" | "editions") {
        return Err(CoreError::Config(format!("invalid duplicate decision: {decision}")));
    }
    let conn = db.writer();
    let now = chrono::Utc::now().timestamp();
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
            tx.execute(
                "UPDATE duplicate_evaluated_pairs SET decision = ?1, evaluated_at = ?2
                 WHERE doc_id_min = ?3 AND doc_id_max = ?4",
                params![decision, now, doc_ids[i], doc_ids[j]],
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
}
