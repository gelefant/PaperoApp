use crate::error::Result;
use crate::index::vector::VectorIndex;
use crate::metadata::extract::extract_doi_from_text;
use crate::metadata::models::{normalize_arxiv_base_id, normalize_doi, normalize_mr_digits};
use crate::ml::embedder::l2_normalize;
use crate::text_norm::normalize_math_text;
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarDocumentItem {
    pub document_id: i64,
    pub file_id: i64,
    pub title: String,
    pub authors_summary: String,
    pub year: Option<i64>,
    pub work_type: String,
    pub similarity: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationLinkItem {
    pub document_id: i64,
    pub file_id: i64,
    pub title: String,
    pub authors_summary: String,
    pub year: Option<i64>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreprintPublishedNotice {
    pub doi: Option<String>,
    pub mr_number: Option<String>,
    pub journal: Option<String>,
    pub year: Option<i64>,
    pub title: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentRelationsView {
    pub similar: Vec<SimilarDocumentItem>,
    pub cites: Vec<CitationLinkItem>,
    pub cited_by: Vec<CitationLinkItem>,
    pub published_notice: Option<PreprintPublishedNotice>,
}

/// Computes top-K similar documents on-the-fly directly from the already-mmapped HNSW `usearch`
/// passage index (zero extra indexing, `< 5ms` even on large libraries).
pub fn find_similar_documents(
    conn: &Connection,
    vindex: &VectorIndex,
    document_id: i64,
    limit: usize,
) -> Result<Vec<SimilarDocumentItem>> {
    if limit == 0 || vindex.is_empty() {
        return Ok(Vec::new());
    }

    // Sample up to 24 evenly distributed / leading passage IDs via covering index `idx_passages_doc_page`.
    let target_pids: Vec<u64> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM passages
             WHERE document_id = ?1
             ORDER BY page_ord ASC, chunk_index ASC
             LIMIT 24",
        )?;
        let rows = stmt.query_map(params![document_id], |r| Ok(r.get::<_, i64>(0)? as u64))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    if target_pids.is_empty() {
        return Ok(Vec::new());
    }

    let dim = vindex.dimensions();
    let mut sum = vec![0.0_f32; dim];
    let mut scratch = vec![0.0_f32; dim];
    let mut count = 0usize;
    for pid in target_pids {
        if vindex.read_vector_into(pid, &mut scratch) {
            for (dst, src) in sum.iter_mut().zip(scratch.iter()) {
                *dst += *src;
            }
            count += 1;
        }
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    let inv = 1.0 / (count as f32);
    for v in &mut sum {
        *v *= inv;
    }
    l2_normalize(&mut sum);

    let search_k = (limit * 24).clamp(64, 180);
    let hits = vindex.search(&sum, search_k)?;
    if hits.is_empty() {
        return Ok(Vec::new());
    }

    let pid_list: Vec<i64> = hits.iter().map(|h| h.passage_id as i64).collect();
    let mut pid_to_doc: HashMap<u64, i64> = HashMap::with_capacity(pid_list.len());
    for chunk in pid_list.chunks(500) {
        let ph = (1..=chunk.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT id, document_id FROM passages WHERE id IN ({ph})");
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> =
            chunk.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(refs.as_slice(), |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?))
        })?;
        for r in rows {
            let (pid, did) = r?;
            pid_to_doc.insert(pid, did);
        }
    }

    // Aggregate top-2 passage cosine scores per candidate document (excluding self)
    let mut doc_scores: HashMap<i64, Vec<f32>> = HashMap::new();
    for h in hits {
        if let Some(&did) = pid_to_doc.get(&h.passage_id) {
            if did == document_id || h.score <= 0.05 {
                continue;
            }
            doc_scores.entry(did).or_default().push(h.score);
        }
    }

    let mut ranked: Vec<(i64, f32)> = doc_scores
        .into_iter()
        .map(|(did, mut scores)| {
            scores.sort_by(|a, b| b.total_cmp(a));
            let agg = if scores.len() >= 2 {
                0.65 * scores[0] + 0.35 * scores[1]
            } else {
                scores[0]
            };
            (did, agg)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(limit);

    if ranked.is_empty() {
        return Ok(Vec::new());
    }

    let mut hydrated = hydrate_brief_docs(conn, &ranked.iter().map(|(id, _)| *id).collect::<Vec<_>>())?;
    let mut out = Vec::with_capacity(ranked.len());
    for (did, sim) in ranked {
        if let Some((file_id, title, authors_summary, year, work_type)) = hydrated.remove(&did) {
            out.push(SimilarDocumentItem {
                document_id: did,
                file_id,
                title,
                authors_summary,
                year,
                work_type,
                similarity: sim.clamp(0.0, 1.0),
            });
        }
    }
    Ok(out)
}

fn ref_doi_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)\b10\.\d{4,9}/[^\s"',;\]\)>]+"#).unwrap())
}

fn ref_arxiv_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:\barXiv:\s*|arxiv\.org/(?:abs|pdf)/)(\d{4}\.\d{4,5}|[a-z-]+(?:\.[A-Z]{2})?/\d{7})(?:v\d+)?\b",
        )
        .unwrap()
    })
}

fn ref_mr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bMR0*(\d{5,7})\b").unwrap())
}

#[derive(Default)]
struct LibraryIdLookup {
    by_doi: HashMap<String, i64>,
    by_arxiv: HashMap<String, i64>,
    by_mr: HashMap<String, i64>,
}

fn load_library_id_lookup(conn: &Connection) -> Result<LibraryIdLookup> {
    let mut lut = LibraryIdLookup::default();
    let mut stmt = conn.prepare(
        "SELECT id, doi, arxiv_id, mr_number
         FROM documents
         WHERE doi IS NOT NULL OR arxiv_id IS NOT NULL OR mr_number IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;
    for row in rows {
        let (id, doi, arxiv, mr) = row?;
        if let Some(d) = doi.as_deref().and_then(normalize_doi) {
            lut.by_doi.entry(d).or_insert(id);
        }
        if let Some(ax) = arxiv.as_deref().and_then(normalize_arxiv_base_id) {
            lut.by_arxiv.entry(ax).or_insert(id);
        }
        if let Some(m) = mr.as_deref().and_then(normalize_mr_digits) {
            lut.by_mr.entry(m).or_insert(id);
        }
    }
    Ok(lut)
}

///Read-only discovery of local outgoing citations for `document_id` (suitable for `db.reader()`).
fn discover_local_citations_for_doc(
    conn: &Connection,
    document_id: i64,
) -> Result<(Vec<(i64, &'static str)>, usize)> {
    let doc_ids_row: Option<(Option<String>, Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT d.doi, d.arxiv_id, MAX(f.page_count)
             FROM documents d
             LEFT JOIN files f ON f.document_id = d.id
             WHERE d.id = ?1
             GROUP BY d.id",
            params![document_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((doc_doi, doc_arxiv, page_count_opt)) = doc_ids_row else {
        return Ok((Vec::new(), 0));
    };

    let lut = load_library_id_lookup(conn)?;
    if lut.by_doi.is_empty() && lut.by_arxiv.is_empty() && lut.by_mr.is_empty() {
        return Ok((Vec::new(), 0));
    }

    let existing_cited: HashSet<i64> = {
        let mut stmt =
            conn.prepare_cached("SELECT cited_doc_id FROM document_citations WHERE citing_doc_id = ?1")?;
        let rows = stmt.query_map(params![document_id], |r| r.get::<_, i64>(0))?;
        rows.collect::<std::result::Result<HashSet<_>, _>>()?
    };

    let mut new_edges: Vec<(i64, &'static str)> = Vec::new();
    let mut seen_cited: HashSet<i64> = HashSet::new();
    let mut total_found = 0usize;

    // 1. Inspect cached Crossref / Semantic Scholar / OpenAlex JSON for this doc's DOI or arXiv
    let mut cache_keys: Vec<(&str, String)> = Vec::new();
    if let Some(d) = doc_doi.as_deref().and_then(normalize_doi) {
        cache_keys.push(("crossref", format!("doi:{d}")));
        cache_keys.push(("openalex", format!("doi:{d}")));
        cache_keys.push(("semanticscholar", format!("doi:{d}")));
    }
    if let Some(ax) = doc_arxiv.as_deref().and_then(normalize_arxiv_base_id) {
        cache_keys.push(("semanticscholar", format!("arxiv:{ax}")));
    }
    for (src, key) in cache_keys {
        let body_opt: Option<String> = conn
            .query_row(
                "SELECT response_body FROM metadata_http_cache
                 WHERE source = ?1 AND lookup_key = ?2 AND status_code < 400",
                params![src, key],
                |r| r.get(0),
            )
            .optional()?;
        let Some(body) = body_opt else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            continue;
        };
        if src == "crossref" {
            if let Some(refs) = v
                .get("message")
                .and_then(|m| m.get("reference"))
                .and_then(Value::as_array)
            {
                for r in refs {
                    if let Some(d) = r.get("DOI").and_then(Value::as_str).and_then(normalize_doi) {
                        if let Some(&cited_id) = lut.by_doi.get(&d) {
                            if cited_id != document_id && seen_cited.insert(cited_id) {
                                total_found += 1;
                                if !existing_cited.contains(&cited_id) {
                                    new_edges.push((cited_id, "crossref"));
                                }
                            }
                        }
                    }
                }
            }
        } else if src == "semanticscholar" {
            if let Some(refs) = v.get("references").and_then(Value::as_array) {
                for r in refs {
                    let ext = r.get("externalIds");
                    if let Some(d) = ext
                        .and_then(|e| e.get("DOI"))
                        .and_then(Value::as_str)
                        .and_then(normalize_doi)
                    {
                        if let Some(&cited_id) = lut.by_doi.get(&d) {
                            if cited_id != document_id && seen_cited.insert(cited_id) {
                                total_found += 1;
                                if !existing_cited.contains(&cited_id) {
                                    new_edges.push((cited_id, "semanticscholar"));
                                }
                            }
                        }
                    }
                    if let Some(ax) = ext
                        .and_then(|e| e.get("ArXiv"))
                        .and_then(Value::as_str)
                        .and_then(normalize_arxiv_base_id)
                    {
                        if let Some(&cited_id) = lut.by_arxiv.get(&ax) {
                            if cited_id != document_id && seen_cited.insert(cited_id) {
                                total_found += 1;
                                if !existing_cited.contains(&cited_id) {
                                    new_edges.push((cited_id, "semanticscholar"));
                                }
                            }
                        }
                    }
                }
            }
        } else if src == "openalex" && !lut.by_doi.is_empty() {
            // OpenAlex stores `referenced_works: ["https://openalex.org/W..."]`.
            // Cross-reference against any cached OpenAlex work IDs belonging to local library DOIs.
            if let Some(ref_works) = v.get("referenced_works").and_then(Value::as_array) {
                let ref_set: HashSet<&str> =
                    ref_works.iter().filter_map(Value::as_str).collect();
                if !ref_set.is_empty() {
                    let mut c_stmt = conn.prepare_cached(
                        "SELECT lookup_key, response_body FROM metadata_http_cache
                         WHERE source = 'openalex' AND lookup_key LIKE 'doi:%' AND status_code < 400",
                    )?;
                    let c_rows = c_stmt.query_map([], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?;
                    for cr in c_rows.flatten() {
                        let (lk, rb) = cr;
                        let Some(d) = lk.strip_prefix("doi:").and_then(normalize_doi) else {
                            continue;
                        };
                        let Some(&cited_id) = lut.by_doi.get(&d) else {
                            continue;
                        };
                        if cited_id == document_id || seen_cited.contains(&cited_id) {
                            continue;
                        }
                        // Fast substring pre-filter avoids full JSON parsing on 99%+ of cached works.
                        if !ref_set.iter().any(|wid| rb.contains(wid)) {
                            continue;
                        }
                        if let Ok(other_v) = serde_json::from_str::<Value>(&rb) {
                            if other_v
                                .get("id")
                                .and_then(Value::as_str)
                                .is_some_and(|wid| ref_set.contains(wid))
                            {
                                seen_cited.insert(cited_id);
                                total_found += 1;
                                if !existing_cited.contains(&cited_id) {
                                    new_edges.push((cited_id, "openalex"));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Scan trailing bibliography passages for DOI / arXiv / MR identifiers
    let page_count = page_count_opt.unwrap_or(0).max(0);
    let min_ref_page = if page_count >= 12 {
        (page_count * 70) / 100
    } else {
        1
    };
    let mut p_stmt = conn.prepare_cached(
        "SELECT text_content FROM passages
         WHERE document_id = ?1 AND page_ord >= ?2
         ORDER BY page_ord DESC, chunk_index DESC
         LIMIT 36",
    )?;
    let p_rows = p_stmt.query_map(params![document_id, min_ref_page], |r| r.get::<_, String>(0))?;
    for txt in p_rows.flatten() {
        for m in ref_doi_re().find_iter(&txt) {
            if let Some(d) = normalize_doi(m.as_str()) {
                if let Some(&cited_id) = lut.by_doi.get(&d) {
                    if cited_id != document_id && seen_cited.insert(cited_id) {
                        total_found += 1;
                        if !existing_cited.contains(&cited_id) {
                            new_edges.push((cited_id, "extracted_ref"));
                        }
                    }
                }
            }
        }
        for cap in ref_arxiv_re().captures_iter(&txt) {
            if let Some(ax) = cap.get(1).and_then(|m| normalize_arxiv_base_id(m.as_str())) {
                if let Some(&cited_id) = lut.by_arxiv.get(&ax) {
                    if cited_id != document_id && seen_cited.insert(cited_id) {
                        total_found += 1;
                        if !existing_cited.contains(&cited_id) {
                            new_edges.push((cited_id, "extracted_ref"));
                        }
                    }
                }
            }
        }
        for cap in ref_mr_re().captures_iter(&txt) {
            if let Some(mr) = cap.get(1).and_then(|m| normalize_mr_digits(m.as_str())) {
                if let Some(&cited_id) = lut.by_mr.get(&mr) {
                    if cited_id != document_id && seen_cited.insert(cited_id) {
                        total_found += 1;
                        if !existing_cited.contains(&cited_id) {
                            new_edges.push((cited_id, "extracted_ref"));
                        }
                    }
                }
            }
        }
    }

    Ok((new_edges, total_found))
}

fn persist_discovered_citations(
    conn: &Connection,
    document_id: i64,
    new_edges: &[(i64, &'static str)],
) -> Result<()> {
    if new_edges.is_empty() {
        return Ok(());
    }
    let mut ins = conn.prepare_cached(
        "INSERT OR IGNORE INTO document_citations (citing_doc_id, cited_doc_id, source, confidence)
         VALUES (?1, ?2, ?3, 1.0)",
    )?;
    for (cited_id, src) in new_edges {
        ins.execute(params![document_id, cited_id, src])?;
    }
    Ok(())
}

/// Scans bibliography passages + cached OpenAlex/Crossref/Semantic Scholar HTTP payloads for
/// `document_id` and records local outgoing citations in `document_citations`.
pub fn refresh_local_citations_for_doc(conn: &Connection, document_id: i64) -> Result<usize> {
    let (new_edges, total_found) = discover_local_citations_for_doc(conn, document_id)?;
    persist_discovered_citations(conn, document_id, &new_edges)?;
    Ok(total_found)
}

/// Reader-first variant of `refresh_local_citations_for_doc` that avoids locking `db.writer()`
/// unless newly discovered citation edges actually need to be inserted.
pub fn refresh_local_citations_on_db(
    db: &crate::db::Database,
    document_id: i64,
) -> Result<usize> {
    let (new_edges, total_found) = {
        let r = db.reader();
        discover_local_citations_for_doc(&r, document_id)?
    };
    if !new_edges.is_empty() {
        let w = db.writer();
        persist_discovered_citations(&w, document_id, &new_edges)?;
    }
    Ok(total_found)
}

struct PreprintDocRow {
    arxiv_raw: Option<String>,
    doi_raw: Option<String>,
    mr_raw: Option<String>,
    cached_notice_json: Option<String>,
    title_norm: String,
}

struct PreprintSiblingRow {
    doi: Option<String>,
    mr: Option<String>,
    journal: Option<String>,
    year: Option<i64>,
    title: String,
}

/// Checks if an arXiv preprint in the library has a published journal version with DOI/MR
/// based on sibling library records or cached bibliographic API payloads (zero extra network calls).
pub fn detect_preprint_published_notice(
    conn: &Connection,
    document_id: i64,
) -> Result<Option<PreprintPublishedNotice>> {
    let row = conn
        .query_row(
            "SELECT arxiv_id, doi, mr_number, published_notice_json, title_norm
             FROM documents WHERE id = ?1",
            params![document_id],
            |r| {
                Ok(PreprintDocRow {
                    arxiv_raw: r.get(0)?,
                    doi_raw: r.get(1)?,
                    mr_raw: r.get(2)?,
                    cached_notice_json: r.get(3)?,
                    title_norm: r.get(4)?,
                })
            },
        )
        .optional()?;
    let Some(PreprintDocRow {
        arxiv_raw,
        doi_raw,
        mr_raw,
        cached_notice_json,
        title_norm,
    }) = row
    else {
        return Ok(None);
    };

    let Some(arxiv_base) = arxiv_raw.as_deref().and_then(normalize_arxiv_base_id) else {
        return Ok(None);
    };

    // Already has a non-arXiv published DOI or an MR number -> not an unpublished preprint!
    let has_published_doi = doi_raw
        .as_deref()
        .and_then(normalize_doi)
        .is_some_and(|d| !d.starts_with("10.48550/arxiv."));
    let has_mr = mr_raw.as_deref().is_some_and(|s| !s.trim().is_empty());
    if has_published_doi || has_mr {
        return Ok(None);
    }

    if let Some(json) = cached_notice_json.as_deref().filter(|s| !s.trim().is_empty()) {
        if let Ok(parsed) = serde_json::from_str::<PreprintPublishedNotice>(json) {
            return Ok(Some(parsed));
        }
    }

    // 1. Check sibling record in the local library sharing the same arXiv ID
    let sibling = conn
        .query_row(
            "SELECT doi, mr_number, COALESCE(journal_abbrev, journal_full), year, title
             FROM documents
             WHERE id != ?1
               AND LOWER(COALESCE(arxiv_id, '')) = ?2
               AND (
                    (doi IS NOT NULL AND LOWER(doi) NOT LIKE '10.48550/arxiv.%')
                    OR mr_number IS NOT NULL
               )
             LIMIT 1",
            params![document_id, arxiv_base],
            |r| {
                Ok(PreprintSiblingRow {
                    doi: r.get(0)?,
                    mr: r.get(1)?,
                    journal: r.get(2)?,
                    year: r.get(3)?,
                    title: r.get(4)?,
                })
            },
        )
        .optional()?;
    if let Some(PreprintSiblingRow {
        doi,
        mr,
        journal,
        year,
        title,
    }) = sibling
    {
        return Ok(Some(PreprintPublishedNotice {
            doi,
            mr_number: mr,
            journal,
            year,
            title: Some(title),
            source: "library".to_string(),
        }));
    }

    // 2. Check cached arXiv Atom XML (`<arxiv:doi>`)
    if let Some(xml) = load_cached_body(conn, "arxiv", &format!("id:{arxiv_base}"))? {
        if let Some(doi) = extract_xml_tag_value(&xml, "arxiv:doi").and_then(|d| normalize_doi(&d)) {
            if !doi.starts_with("10.48550/arxiv.") {
                let journal = extract_xml_tag_value(&xml, "arxiv:journal_ref");
                return Ok(Some(PreprintPublishedNotice {
                    doi: Some(doi),
                    mr_number: None,
                    journal,
                    year: None,
                    title: None,
                    source: "arxiv".to_string(),
                }));
            }
        }
    }

    // 3. Check cached Semantic Scholar entry for `arxiv:<id>`
    if let Some(body) = load_cached_body(conn, "semanticscholar", &format!("arxiv:{arxiv_base}"))? {
        if let Ok(v) = serde_json::from_str::<Value>(&body) {
            let doi = v
                .get("externalIds")
                .and_then(|e| e.get("DOI"))
                .and_then(Value::as_str)
                .and_then(normalize_doi)
                .filter(|d| !d.starts_with("10.48550/arxiv."));
            if doi.is_some() {
                let journal = v
                    .get("journal")
                    .and_then(|j| j.get("name"))
                    .and_then(Value::as_str)
                    .or_else(|| v.get("venue").and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned);
                let year = v.get("year").and_then(Value::as_i64);
                let title = v.get("title").and_then(Value::as_str).map(ToOwned::to_owned);
                return Ok(Some(PreprintPublishedNotice {
                    doi,
                    mr_number: None,
                    journal,
                    year,
                    title,
                    source: "semanticscholar".to_string(),
                }));
            }
        }
    }

    // 4. Check cached OpenAlex search/title entry for this title
    if !title_norm.trim().is_empty() {
        for key in [format!("q:{title_norm}"), format!("ts:{title_norm}")] {
            if let Some(body) = load_cached_body(conn, "openalex", &key)? {
                if let Ok(v) = serde_json::from_str::<Value>(&body) {
                    if let Some(items) = v.get("results").and_then(Value::as_array) {
                        for item in items.iter().take(3) {
                            let doi = item
                                .get("doi")
                                .and_then(Value::as_str)
                                .and_then(normalize_doi)
                                .filter(|d| !d.starts_with("10.48550/arxiv."));
                            let mentions_arxiv = item
                                .get("locations")
                                .and_then(Value::as_array)
                                .is_some_and(|locs| {
                                    locs.iter().any(|l| {
                                        l.get("landing_page_url")
                                            .and_then(Value::as_str)
                                            .is_some_and(|u| u.to_ascii_lowercase().contains(&arxiv_base))
                                    })
                                });
                            if doi.is_some() && mentions_arxiv {
                                let journal = item
                                    .get("primary_location")
                                    .and_then(|pl| pl.get("source"))
                                    .and_then(|s| s.get("display_name"))
                                    .and_then(Value::as_str)
                                    .map(ToOwned::to_owned);
                                let year = item.get("publication_year").and_then(Value::as_i64);
                                let title = item.get("title").and_then(Value::as_str).map(ToOwned::to_owned);
                                return Ok(Some(PreprintPublishedNotice {
                                    doi,
                                    mr_number: None,
                                    journal,
                                    year,
                                    title,
                                    source: "openalex".to_string(),
                                }));
                            }
                        }
                    }
                }
            }
        }
    }

    // 5. Also check pending metadata candidates for a published DOI / MR!
    let cand_opt: Option<(String, String)> = conn
        .query_row(
            "SELECT source, candidate_json FROM metadata_candidates
             WHERE document_id = ?1 ORDER BY match_score DESC LIMIT 5",
            params![document_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((src, cjson)) = cand_opt {
        if let Ok(rec) = serde_json::from_str::<crate::metadata::models::BibliographicRecord>(&cjson)
        {
            let doi = rec
                .doi
                .as_deref()
                .and_then(normalize_doi)
                .filter(|d| !d.starts_with("10.48550/arxiv."));
            let mr = rec.mr_number.as_deref().and_then(normalize_mr_digits);
            if doi.is_some() || mr.is_some() {
                return Ok(Some(PreprintPublishedNotice {
                    doi,
                    mr_number: mr,
                    journal: rec.journal_abbrev.or(rec.journal_full),
                    year: rec.year,
                    title: Some(rec.title),
                    source: src,
                }));
            }
        }
    }

    // 6. Check if a non-arXiv DOI appeared inside cached zbMATH / MRLookup by arXiv
    if let Some(body) = load_cached_body(conn, "zbmath", &format!("arxiv:{arxiv_base}"))? {
        if let Some(doi) = extract_doi_from_text(&body).filter(|d| !d.starts_with("10.48550/arxiv.")) {
            return Ok(Some(PreprintPublishedNotice {
                doi: Some(doi),
                mr_number: None,
                journal: None,
                year: None,
                title: None,
                source: "zbmath".to_string(),
            }));
        }
    }

    Ok(None)
}

fn load_cached_body(conn: &Connection, source: &str, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT response_body FROM metadata_http_cache
         WHERE source = ?1 AND lookup_key = ?2 AND status_code < 400",
        params![source, key],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn extract_xml_tag_value(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start_tag = xml.find(&open)?;
    let content_start = xml[start_tag..].find('>')? + start_tag + 1;
    let content_end = xml[content_start..].find(&close)? + content_start;
    let val = xml[content_start..content_end].trim();
    (!val.is_empty()).then(|| val.to_string())
}

/// Loads similar documents (HNSW), outgoing/incoming local citations, and preprint published notice.
pub fn get_document_relations(
    conn: &Connection,
    vindex: Option<&VectorIndex>,
    document_id: i64,
    similar_limit: usize,
) -> Result<DocumentRelationsView> {
    let similar = match vindex {
        Some(vi) => find_similar_documents(conn, vi, document_id, similar_limit)?,
        None => Vec::new(),
    };

    let cites_ids: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT cited_doc_id, source FROM document_citations WHERE citing_doc_id = ?1 ORDER BY cited_doc_id ASC",
        )?;
        let rows = stmt.query_map(params![document_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let cited_by_ids: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT citing_doc_id, source FROM document_citations WHERE cited_doc_id = ?1 ORDER BY citing_doc_id ASC",
        )?;
        let rows = stmt.query_map(params![document_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    let mut all_ids: Vec<i64> = cites_ids
        .iter()
        .chain(cited_by_ids.iter())
        .map(|(id, _)| *id)
        .collect();
    all_ids.sort_unstable();
    all_ids.dedup();
    let meta_map = hydrate_brief_docs(conn, &all_ids)?;

    let to_links = |pairs: Vec<(i64, String)>| -> Vec<CitationLinkItem> {
        pairs
            .into_iter()
            .filter_map(|(did, src)| {
                let (file_id, title, authors_summary, year, _) = meta_map.get(&did)?;
                Some(CitationLinkItem {
                    document_id: did,
                    file_id: *file_id,
                    title: title.clone(),
                    authors_summary: authors_summary.clone(),
                    year: *year,
                    source: src,
                })
            })
            .collect()
    };

    let published_notice = detect_preprint_published_notice(conn, document_id)?;

    Ok(DocumentRelationsView {
        similar,
        cites: to_links(cites_ids),
        cited_by: to_links(cited_by_ids),
        published_notice,
    })
}

type BriefDocTuple = (i64, String, String, Option<i64>, String);

fn hydrate_brief_docs(
    conn: &Connection,
    doc_ids: &[i64],
) -> Result<HashMap<i64, BriefDocTuple>> {
    let mut out = HashMap::new();
    if doc_ids.is_empty() {
        return Ok(out);
    }
    for chunk in doc_ids.chunks(500) {
        let ph = (1..=chunk.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT d.id, MIN(f.id), d.title,
                    COALESCE(
                        (SELECT GROUP_CONCAT(
                            CASE WHEN TRIM(given_name) = '' THEN family_name
                                 WHEN TRIM(family_name) = '' THEN given_name
                                 ELSE family_name || ', ' || given_name END,
                            '; '
                         )
                         FROM (
                            SELECT family_name, given_name
                            FROM document_authors
                            WHERE document_id = d.id
                            ORDER BY role ASC, position ASC
                         )),
                        NULLIF(d.authors_norm_summary, ''),
                        ''
                    ),
                    d.year, d.work_type
             FROM documents d
             JOIN files f ON f.document_id = d.id
             WHERE d.id IN ({ph})
             GROUP BY d.id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> =
            chunk.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(refs.as_slice(), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                (
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, String>(5)?,
                ),
            ))
        })?;
        for r in rows {
            let (did, tuple) = r?;
            out.insert(did, tuple);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingClassicCiter {
    pub document_id: i64,
    pub title: String,
    pub year: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingClassicItem {
    pub title: String,
    pub authors: String,
    pub year: Option<i64>,
    pub venue: Option<String>,
    /// `"paper"` | `"book"`
    pub work_category: String,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub isbn: Option<String>,
    pub cited_by_count: usize,
    pub citing_docs: Vec<MissingClassicCiter>,
}

#[derive(Default)]
struct MissingRefAgg {
    title: String,
    authors: String,
    year: Option<i64>,
    venue: Option<String>,
    is_book_votes: usize,
    is_paper_votes: usize,
    doi: Option<String>,
    arxiv_id: Option<String>,
    isbn: Option<String>,
    citers: HashSet<i64>,
}

fn looks_like_book_reference(
    has_isbn: bool,
    has_article_title: bool,
    volume_title: Option<&str>,
    journal_title: Option<&str>,
    venue: Option<&str>,
    title: &str,
) -> bool {
    if has_isbn {
        return true;
    }
    if !has_article_title && volume_title.is_some_and(|s| !s.trim().is_empty()) && journal_title.is_none() {
        return true;
    }
    let combined = format!(
        "{} {}",
        title.to_ascii_lowercase(),
        venue.unwrap_or("").to_ascii_lowercase()
    );
    const BOOK_MARKERS: &[&str] = &[
        "graduate texts in mathematics",
        "lecture notes in mathematics",
        "grundlehren der mathematischen",
        "cambridge university press",
        "oxford university press",
        "princeton university press",
        "american mathematical society",
        "springer-verlag",
        "springer verlag",
        "academic press",
        "birkhäuser",
        "birkhauser",
        "wiley",
        "addison-wesley",
        "mcgraw-hill",
        "chelsea publishing",
        "dover publications",
        "monograph",
        "textbook",
        "2nd ed",
        "3rd ed",
        "second edition",
        "third edition",
    ];
    BOOK_MARKERS.iter().any(|m| combined.contains(m))
}

fn ref_isbn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bISBN(?:-1[03])?:?\s*([0-9Xx][0-9Xx -]{8,16}[0-9Xx])\b").unwrap())
}

fn contains_word_subslice(haystack: &[String], needle: &[String]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn extract_all_years_from_str(raw: &str) -> HashSet<i64> {
    let mut out = HashSet::new();
    let bytes = raw.as_bytes();
    for win in bytes.windows(4) {
        if win.iter().all(|b| b.is_ascii_digit()) {
            if let Ok(s) = std::str::from_utf8(win) {
                if let Ok(y) = s.parse::<i64>() {
                    if (1700..=2100).contains(&y) {
                        out.insert(y);
                    }
                }
            }
        }
    }
    out
}

/// Resolves a raw bibliography entry snippet (extracted near a PDF internal `\cite{...}` target)
/// against the local library offline (`< 5 ms`), returning the matching owned [`CitationLinkItem`]
/// when a high-confidence match exists (DOI, arXiv, MR, ISBN-10/13, or title + author + year).
pub fn resolve_citation_snippet_in_library(
    conn: &Connection,
    citing_document_id: Option<i64>,
    snippet: &str,
) -> Result<Option<CitationLinkItem>> {
    let trimmed = snippet.trim();
    if trimmed.len() < 8 {
        return Ok(None);
    }
    let bounded: String = trimmed.chars().take(1600).collect();

    let finish = |doc_id: i64, source: &str| -> Result<Option<CitationLinkItem>> {
        if citing_document_id == Some(doc_id) {
            return Ok(None);
        }
        let mut map = hydrate_brief_docs(conn, &[doc_id])?;
        let Some((file_id, title, authors_summary, year, _)) = map.remove(&doc_id) else {
            return Ok(None);
        };
        if file_id <= 0 {
            return Ok(None);
        }
        Ok(Some(CitationLinkItem {
            document_id: doc_id,
            file_id,
            title,
            authors_summary,
            year,
            source: source.to_string(),
        }))
    };

    // 1. Exact identifier matching (DOI / book-chapter DOI stem, arXiv, MR, ISBN-10/13/ISBN-9)
    let lut = load_library_id_lookup(conn)?;
    for m in ref_doi_re().find_iter(&bounded) {
        if let Some(d) = normalize_doi(m.as_str()) {
            let hit = lut
                .by_doi
                .get(&d)
                .copied()
                .or_else(|| doi_book_stem(&d).and_then(|stem| lut.by_doi.get(&stem).copied()));
            if let Some(doc_id) = hit {
                if let Some(item) = finish(doc_id, "doi")? {
                    return Ok(Some(item));
                }
            }
        }
    }
    for cap in ref_arxiv_re().captures_iter(&bounded) {
        if let Some(ax) = normalize_arxiv_base_id(&cap[1]) {
            if let Some(&doc_id) = lut.by_arxiv.get(&ax) {
                if let Some(item) = finish(doc_id, "arxiv")? {
                    return Ok(Some(item));
                }
            }
        }
    }
    for cap in ref_mr_re().captures_iter(&bounded) {
        if let Some(mr) = normalize_mr_digits(&cap[1]) {
            if let Some(&doc_id) = lut.by_mr.get(&mr) {
                if let Some(item) = finish(doc_id, "mr")? {
                    return Ok(Some(item));
                }
            }
        }
    }
    for cap in ref_isbn_re().captures_iter(&bounded) {
        if let Some(norm_isbn) = normalize_isbn_digits(&cap[1]) {
            let core9 = isbn9_core(&norm_isbn);
            let mut stmt = conn.prepare(
                "SELECT id, isbn FROM documents WHERE isbn IS NOT NULL AND TRIM(isbn) != ''",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for r in rows {
                let (doc_id, raw_doc_isbn) = r?;
                if let Some(doc_isbn) = normalize_isbn_digits(&raw_doc_isbn) {
                    let same = doc_isbn == norm_isbn
                        || (core9.is_some() && isbn9_core(&doc_isbn) == core9);
                    if same {
                        if let Some(item) = finish(doc_id, "isbn")? {
                            return Ok(Some(item));
                        }
                    }
                }
            }
        }
    }

    // 2. Fuzzy structured match against local library titles + author surnames + years
    //    (boosted by pre-linked `document_citations` edges when present).
    let snippet_words = tokenize_alnum_words(&bounded);
    if snippet_words.len() < 3 {
        return Ok(None);
    }
    let snippet_word_set: HashSet<&str> = snippet_words.iter().map(String::as_str).collect();
    let snippet_years = extract_all_years_from_str(&bounded);

    let known_cited: HashSet<i64> = if let Some(cid) = citing_document_id {
        let mut stmt =
            conn.prepare("SELECT cited_doc_id FROM document_citations WHERE citing_doc_id = ?1")?;
        let rows = stmt.query_map(params![cid], |r| r.get::<_, i64>(0))?;
        rows.filter_map(std::result::Result::ok).collect()
    } else {
        HashSet::new()
    };

    let mut stmt = conn.prepare(
        "SELECT d.id, d.title, d.year, d.work_type,
                COALESCE(GROUP_CONCAT(da.family_name, '|'), '')
         FROM documents d
         LEFT JOIN document_authors da ON da.document_id = d.id AND da.role = 'author'
         WHERE EXISTS (SELECT 1 FROM files f WHERE f.document_id = d.id)
         GROUP BY d.id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;

    let mut best: Option<(i32, i64, &'static str)> = None;

    for row in rows {
        let (doc_id, doc_title, doc_year, work_type, authors_concat) = row?;
        if citing_document_id == Some(doc_id) {
            continue;
        }
        let mut title_words = tokenize_alnum_words(&doc_title);
        if title_words.len() >= 2 && is_leading_article(&title_words[0]) {
            title_words.remove(0);
        }
        if title_words.is_empty() {
            continue;
        }

        // Prevent Part I / Part II / Part III / Part IV cross-matching in BOTH directions
        let part_conflict = ["ii", "iii", "iv", "v", "vi"].iter().any(|pt| {
            title_words.iter().any(|w| w == pt) != snippet_word_set.contains(pt)
        }) || (title_words.last().is_some_and(|w| w == "i") && !snippet_word_set.contains("i"));
        if part_conflict {
            continue;
        }

        let sig_words: Vec<&str> = title_words
            .iter()
            .map(String::as_str)
            .filter(|w| w.len() >= 3 && !is_title_stopword(w))
            .collect();
        if sig_words.is_empty() {
            continue;
        }

        let matched_sig = sig_words
            .iter()
            .filter(|&&w| snippet_word_set.contains(w))
            .count();

        let contiguous = contains_word_subslice(&snippet_words, &title_words);
        let title_char_len: usize = title_words.iter().map(String::len).sum();
        let high_sig_overlap = sig_words.len() >= 4
            && matched_sig + usize::from(sig_words.len() >= 6) >= sig_words.len();

        if !contiguous && !high_sig_overlap {
            continue;
        }

        // Check author surname match
        let author_tokens: Vec<String> = authors_concat
            .split('|')
            .filter(|s| !s.trim().is_empty())
            .flat_map(tokenize_alnum_words)
            .filter(|w| w.len() >= 3)
            .collect();
        let has_authors = !author_tokens.is_empty();
        let author_matched = author_tokens
            .iter()
            .any(|a| snippet_word_set.contains(a.as_str()));

        // Check year compatibility
        let year_delta: Option<i64> = match (doc_year, snippet_years.is_empty()) {
            (Some(dy), false) => snippet_years.iter().map(|sy| (sy - dy).abs()).min(),
            _ => None,
        };
        let is_book = work_type == "book";
        let year_ok = match year_delta {
            Some(d) => d <= if is_book { 15 } else { 2 },
            None => true,
        };
        if !year_ok {
            continue;
        }

        let in_doc_citations = known_cited.contains(&doc_id);

        // Require sufficient corroborating evidence for short titles
        let accepted = if title_words.len() >= 4 && contiguous {
            author_matched || year_delta.is_some_and(|d| d <= 1) || in_doc_citations || !has_authors
        } else if contiguous && title_words.len() >= 2 && title_char_len >= 11 {
            author_matched && (year_delta.is_some() || in_doc_citations || is_book)
        } else {
            high_sig_overlap && author_matched && (year_delta.is_some() || in_doc_citations)
        };
        if !accepted {
            continue;
        }

        let mut score: i32 = (matched_sig.min(12) as i32) * 4;
        if contiguous {
            score += 80 + (title_words.len().min(10) as i32) * 3;
        }
        if in_doc_citations {
            score += 45;
        }
        if author_matched {
            score += 35;
        }
        if let Some(d) = year_delta {
            if d == 0 {
                score += 25;
            } else if d <= 2 {
                score += 12;
            }
        }

        let src = if in_doc_citations {
            "citation_graph"
        } else {
            "bibliography_match"
        };
        if best.as_ref().is_none_or(|(best_score, _, _)| score > *best_score) {
            best = Some((score, doc_id, src));
        }
    }

    if let Some((_, doc_id, src)) = best {
        return finish(doc_id, src);
    }
    Ok(None)
}

fn clean_ref_title(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ").trim_matches(['.', ',', ';', '"', '\'']).to_string()
}

fn extract_year_from_str(raw: &str) -> Option<i64> {
    let bytes = raw.as_bytes();
    for win in bytes.windows(4) {
        if win.iter().all(|b| b.is_ascii_digit()) {
            if let Ok(s) = std::str::from_utf8(win) {
                if let Ok(y) = s.parse::<i64>() {
                    if (1700..=2100).contains(&y) {
                        return Some(y);
                    }
                }
            }
        }
    }
    None
}

fn normalize_isbn_digits(raw: &str) -> Option<String> {
    let digits: String = raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if digits.len() == 10 || digits.len() == 13 {
        Some(digits)
    } else {
        None
    }
}

/// Extracts the shared 9-digit ISBN core connecting ISBN-10 (`0387900519` -> `038790051`)
/// and 978-prefixed ISBN-13 (`9780387900513` -> `038790051`).
fn isbn9_core(normalized_isbn: &str) -> Option<String> {
    if normalized_isbn.len() == 10 {
        Some(normalized_isbn[..9].to_string())
    } else if normalized_isbn.len() == 13 && normalized_isbn.starts_with("978") {
        Some(normalized_isbn[3..12].to_string())
    } else {
        None
    }
}

/// Strips common book-chapter DOI suffixes (e.g. `10.1007/978-3-642-66282-9_4` -> `10.1007/978-3-642-66282-9`).
fn doi_book_stem(doi: &str) -> Option<String> {
    let (prefix, suffix) = doi.rsplit_once('_')?;
    if !suffix.is_empty() && suffix.len() <= 4 && suffix.bytes().all(|b| b.is_ascii_digit()) && prefix.contains('/') {
        Some(prefix.to_string())
    } else {
        None
    }
}

fn html_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<[^>]{1,32}>").unwrap())
}

fn tex_cmd_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\\[a-zA-Z]+\*?").unwrap())
}

fn strip_markup(raw: &str) -> String {
    let no_html = html_tag_re().replace_all(raw, " ");
    let no_tex = tex_cmd_re().replace_all(&no_html, " ");
    no_tex.into_owned()
}

fn tokenize_alnum_words(raw: &str) -> Vec<String> {
    let stripped = strip_markup(raw);
    let norm = normalize_math_text(&stripped);
    let mut words = Vec::new();
    let mut cur = String::new();
    for ch in norm.chars() {
        if ch.is_alphanumeric() {
            cur.push(ch);
        } else if !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

fn is_leading_article(w: &str) -> bool {
    matches!(w, "a" | "an" | "the" | "le" | "la" | "les" | "der" | "die" | "das" | "il" | "lo")
}

fn is_title_stopword(w: &str) -> bool {
    matches!(
        w,
        "the"
            | "and"
            | "for"
            | "with"
            | "from"
            | "that"
            | "this"
            | "into"
            | "over"
            | "under"
            | "des"
            | "les"
            | "une"
            | "sur"
            | "dans"
            | "pour"
            | "avec"
            | "der"
            | "die"
            | "das"
            | "und"
            | "von"
            | "zur"
            | "mit"
            | "del"
            | "dei"
            | "degli"
            | "della"
            | "delle"
            | "con"
            | "per"
            | "edition"
            | "second"
            | "third"
            | "fourth"
            | "revised"
            | "2nd"
            | "3rd"
            | "4th"
    )
}

fn clean_canonical_words(raw_words: &[String]) -> Vec<String> {
    let mut slice = raw_words;
    if slice.len() >= 3 && is_leading_article(&slice[0]) {
        slice = &slice[1..];
    }
    let mut out: Vec<String> = slice.to_vec();
    // Strip trailing edition markers like "2nd ed", "second edition", "3rd edition"
    while out.len() >= 3 {
        let last = out[out.len() - 1].as_str();
        let prev = out[out.len() - 2].as_str();
        if matches!(last, "ed" | "edn" | "edition")
            && matches!(
                prev,
                "1st" | "2nd" | "3rd" | "4th" | "5th" | "first" | "second" | "third" | "fourth" | "fifth" | "revised" | "rev" | "corrected"
            )
        {
            out.truncate(out.len() - 2);
        } else {
            break;
        }
    }
    out
}

fn parse_part_num_token(tok: &str) -> Option<&'static str> {
    match tok {
        "1" | "i" => Some("1"),
        "2" | "ii" => Some("2"),
        "3" | "iii" => Some("3"),
        "4" | "iv" => Some("4"),
        "5" | "v" => Some("5"),
        "6" | "vi" => Some("6"),
        "7" | "vii" => Some("7"),
        "8" | "viii" => Some("8"),
        _ => None,
    }
}

/// Detects explicit multi-part / multi-volume suffixes (e.g. "Part II", "Vol. 2", trailing "II"/"III")
/// so Part I and Part II of a paper/book by the same author are never conflated.
fn extract_part_marker(words: &[String]) -> Option<&'static str> {
    for win in words.windows(2) {
        if matches!(win[0].as_str(), "part" | "pt" | "vol" | "volume" | "tome" | "band" | "book") {
            if let Some(n) = parse_part_num_token(&win[1]) {
                return Some(n);
            }
        }
    }
    if words.len() >= 3 {
        if let Some(last) = words.last() {
            if matches!(last.as_str(), "ii" | "iii" | "iv" | "v" | "vi") {
                return parse_part_num_token(last);
            }
        }
    }
    None
}

fn part_markers_compatible(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        (Some(x), None) | (None, Some(x)) => x == "1",
        (None, None) => true,
    }
}

fn significant_word_set(words: &[String]) -> HashSet<String> {
    words
        .iter()
        .filter(|w| w.len() >= 3 && !is_title_stopword(w) && !w.bytes().all(|b| b.is_ascii_digit()))
        .cloned()
        .collect()
}

fn primary_title_before_colon(raw: &str) -> Option<String> {
    let head = raw.split(':').next()?.trim();
    if head.len() == raw.trim().len() {
        return None;
    }
    let words = clean_canonical_words(&tokenize_alnum_words(head));
    let joined = words.join(" ");
    if words.len() >= 2 && joined.len() >= 10 {
        Some(joined)
    } else {
        None
    }
}

fn contains_contiguous_subslice(haystack: &[String], needle: &[String]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn total_words_chars(words: &[String]) -> usize {
    words.iter().map(|w| w.len()).sum()
}

fn extract_surname_tokens(raw: &str) -> HashSet<String> {
    tokenize_alnum_words(raw)
        .into_iter()
        .filter(|w| {
            w.len() >= 3
                && !w.bytes().all(|b| b.is_ascii_digit())
                && !matches!(
                    w.as_str(),
                    "and" | "van" | "von" | "der" | "den" | "del" | "della" | "des" | "les" | "eds" | "editor" | "editors" | "etal"
                )
        })
        .collect()
}

struct OwnedDocEntry {
    words: Vec<String>,
    sig_words: HashSet<String>,
    part_marker: Option<&'static str>,
    surnames: HashSet<String>,
    heuristic_words: Vec<String>,
    heuristic_word_set: HashSet<String>,
    year: Option<i64>,
}

#[derive(Default)]
struct OwnedDocMatcher {
    dois: HashSet<String>,
    doi_stems: HashSet<String>,
    arxivs: HashSet<String>,
    mrs: HashSet<String>,
    isbns: HashSet<String>,
    isbn9_cores: HashSet<String>,
    docs: Vec<OwnedDocEntry>,
    by_canon_title: HashMap<String, Vec<usize>>,
    by_primary_title: HashMap<String, Vec<usize>>,
    by_surname: HashMap<String, Vec<usize>>,
    by_word: HashMap<String, Vec<usize>>,
}

struct RefCheckInput<'a> {
    title: &'a str,
    authors: &'a str,
    unstructured: Option<&'a str>,
    year: Option<i64>,
    doi: Option<&'a str>,
    arxiv_id: Option<&'a str>,
    isbn: Option<&'a str>,
    mr: Option<&'a str>,
}

struct OwnedMatcherBuild {
    matcher: OwnedDocMatcher,
    doc_by_doi: HashMap<String, i64>,
    doc_by_arxiv: HashMap<String, i64>,
    citer_meta: HashMap<i64, (String, Option<i64>)>,
}

impl OwnedDocMatcher {
    fn from_db(conn: &Connection) -> Result<OwnedMatcherBuild> {
        let mut matcher = Self::default();
        let mut doc_by_doi: HashMap<String, i64> = HashMap::new();
        let mut doc_by_arxiv: HashMap<String, i64> = HashMap::new();
        let mut citer_meta: HashMap<i64, (String, Option<i64>)> = HashMap::new();

        // Load structured surnames per document from `document_authors`.
        let mut authors_by_doc: HashMap<i64, HashSet<String>> = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT document_id, family_name FROM document_authors WHERE TRIM(family_name) != ''",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows.flatten() {
                let (did, fam) = row;
                let entry = authors_by_doc.entry(did).or_default();
                for tok in extract_surname_tokens(&fam) {
                    entry.insert(tok);
                }
            }
        }

        // Load file basenames per document so unenriched heuristic files (`Evans - PDE.pdf`) still match.
        let mut file_stems_by_doc: HashMap<i64, Vec<String>> = HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT document_id, rel_path FROM files")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows.flatten() {
                let (did, rel) = row;
                let fname = rel.rsplit(['/', '\\']).next().unwrap_or(&rel);
                let stem = fname
                    .rsplit_once('.')
                    .map(|(s, _)| s)
                    .unwrap_or(fname)
                    .trim();
                if !stem.is_empty() {
                    file_stems_by_doc.entry(did).or_default().push(stem.to_string());
                }
            }
        }

        let mut stmt = conn.prepare(
            "SELECT id, title, authors_norm_summary, year, doi, arxiv_id, isbn, mr_number
             FROM documents",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?;

        for row in rows {
            let (id, title, authors_summary, year, doi, arxiv, isbn, mr) = row?;
            citer_meta.insert(id, (title.clone(), year));

            if let Some(d) = doi.as_deref().and_then(normalize_doi) {
                if let Some(stem) = doi_book_stem(&d) {
                    matcher.doi_stems.insert(stem);
                }
                matcher.dois.insert(d.clone());
                doc_by_doi.entry(d).or_insert(id);
            }
            if let Some(ax) = arxiv.as_deref().and_then(normalize_arxiv_base_id) {
                matcher.arxivs.insert(ax.clone());
                doc_by_arxiv.entry(ax).or_insert(id);
            }
            if let Some(ib) = isbn.as_deref().and_then(normalize_isbn_digits) {
                if let Some(core) = isbn9_core(&ib) {
                    matcher.isbn9_cores.insert(core);
                }
                matcher.isbns.insert(ib);
            }
            if let Some(m) = mr.as_deref().and_then(normalize_mr_digits) {
                matcher.mrs.insert(m);
            }

            let raw_words = tokenize_alnum_words(&title);
            let words = clean_canonical_words(&raw_words);
            let canon_title = words.join(" ");
            let sig_words = significant_word_set(&words);
            let part_marker = extract_part_marker(&raw_words);

            let mut surnames = authors_by_doc.remove(&id).unwrap_or_default();
            if surnames.is_empty() && !authors_summary.trim().is_empty() {
                for seg in authors_summary.split([';', ',']) {
                    let seg_t = seg.trim();
                    if !seg_t.is_empty() {
                        for tok in extract_surname_tokens(seg_t) {
                            surnames.insert(tok);
                        }
                    }
                }
            }

            let mut heuristic_words = raw_words.clone();
            if let Some(stems) = file_stems_by_doc.remove(&id) {
                for stem in stems {
                    heuristic_words.extend(tokenize_alnum_words(&stem));
                }
            }
            let heuristic_word_set: HashSet<String> = heuristic_words.iter().cloned().collect();

            let idx = matcher.docs.len();
            if !canon_title.is_empty() {
                matcher.by_canon_title.entry(canon_title).or_default().push(idx);
            }
            if let Some(prim) = primary_title_before_colon(&title) {
                matcher.by_primary_title.entry(prim).or_default().push(idx);
            }
            for s in &surnames {
                matcher.by_surname.entry(s.clone()).or_default().push(idx);
            }
            for w in &sig_words {
                if w.len() >= 4 {
                    matcher.by_word.entry(w.clone()).or_default().push(idx);
                }
            }
            if surnames.is_empty() {
                for w in &heuristic_word_set {
                    if w.len() >= 4 && !is_title_stopword(w) {
                        let list = matcher.by_word.entry(w.clone()).or_default();
                        if list.last().copied() != Some(idx) {
                            list.push(idx);
                        }
                    }
                }
            }

            matcher.docs.push(OwnedDocEntry {
                words,
                sig_words,
                part_marker,
                surnames,
                heuristic_words,
                heuristic_word_set,
                year,
            });
        }

        Ok(OwnedMatcherBuild {
            matcher,
            doc_by_doi,
            doc_by_arxiv,
            citer_meta,
        })
    }

    fn is_owned(&self, r: &RefCheckInput<'_>) -> bool {
        // 1. Canonical identifier checks (DOI + chapter stem, arXiv, ISBN-10/13, MR)
        if let Some(d) = r.doi {
            if self.dois.contains(d) || self.doi_stems.contains(d) {
                return true;
            }
            if let Some(stem) = doi_book_stem(d) {
                if self.dois.contains(&stem) || self.doi_stems.contains(&stem) {
                    return true;
                }
            }
        }
        if let Some(ax) = r.arxiv_id {
            if self.arxivs.contains(ax) {
                return true;
            }
        }
        if let Some(ib) = r.isbn {
            if self.isbns.contains(ib) {
                return true;
            }
            if let Some(core) = isbn9_core(ib) {
                if self.isbn9_cores.contains(&core) {
                    return true;
                }
            }
        }
        if let Some(m) = r.mr {
            if self.mrs.contains(m) {
                return true;
            }
        }

        let raw_title_words = tokenize_alnum_words(r.title);
        let ref_words = clean_canonical_words(&raw_title_words);
        let ref_canon = ref_words.join(" ");
        if ref_canon.len() < 4 {
            return false;
        }
        let ref_sig = significant_word_set(&ref_words);
        let ref_part = extract_part_marker(&raw_title_words);

        let mut ref_author_tokens = extract_surname_tokens(r.authors);
        let mut full_ref_words = raw_title_words.clone();
        if let Some(unstr) = r.unstructured {
            let u_words = tokenize_alnum_words(unstr);
            for w in &u_words {
                if w.len() >= 3 && self.by_surname.contains_key(w) {
                    ref_author_tokens.insert(w.clone());
                }
            }
            full_ref_words = u_words;
        } else {
            for w in &raw_title_words {
                if w.len() >= 3 && self.by_surname.contains_key(w) {
                    ref_author_tokens.insert(w.clone());
                }
            }
        }

        // 2. Exact canonical title match (protects short single-word titles by requiring author match)
        if let Some(indices) = self.by_canon_title.get(&ref_canon) {
            let strong_title = ref_words.len() >= 2 && ref_canon.len() >= 10;
            for &idx in indices {
                let doc = &self.docs[idx];
                if !part_markers_compatible(doc.part_marker, ref_part) {
                    continue;
                }
                let shares_author = !ref_author_tokens.is_empty()
                    && doc.surnames.iter().any(|s| ref_author_tokens.contains(s));
                let author_conflict = !ref_author_tokens.is_empty()
                    && !doc.surnames.is_empty()
                    && !shares_author
                    && ref_words.len() <= 3;
                if shares_author || (strong_title && !author_conflict) {
                    return true;
                }
            }
        }

        // 3. Primary title before `:` (matches book/paper with or without subtitle)
        let ref_primary = primary_title_before_colon(r.title);
        let primary_candidates: &[String] = match ref_primary {
            Some(ref p) => std::slice::from_ref(p),
            None => std::slice::from_ref(&ref_canon),
        };
        for pkey in primary_candidates {
            if let Some(indices) = self.by_primary_title.get(pkey) {
                for &idx in indices {
                    let doc = &self.docs[idx];
                    if !part_markers_compatible(doc.part_marker, ref_part) {
                        continue;
                    }
                    let shares_author = doc.surnames.iter().any(|s| ref_author_tokens.contains(s));
                    let year_close = match (doc.year, r.year) {
                        (Some(a), Some(b)) => (a - b).abs() <= 2,
                        _ => false,
                    };
                    if shares_author
                        || year_close
                        || (doc.surnames.is_empty() && ref_author_tokens.is_empty() && pkey.len() >= 16)
                    {
                        return true;
                    }
                }
            }
        }
        // Also check when ref has a subtitle and library doc only has the primary title
        if let Some(ref ref_prim) = ref_primary {
            if let Some(indices) = self.by_canon_title.get(ref_prim) {
                for &idx in indices {
                    let doc = &self.docs[idx];
                    if !part_markers_compatible(doc.part_marker, ref_part) {
                        continue;
                    }
                    let shares_author = doc.surnames.iter().any(|s| ref_author_tokens.contains(s));
                    let year_close = match (doc.year, r.year) {
                        (Some(a), Some(b)) => (a - b).abs() <= 2,
                        _ => false,
                    };
                    if shares_author || year_close {
                        return true;
                    }
                }
            }
        }

        // 4. Same-author title containment / high token overlap (catches Crossref `unstructured` citations,
        //    edition suffixes, subtitle omissions, minor punctuation/wording variants).
        if !ref_author_tokens.is_empty() {
            let mut seen_docs: HashSet<usize> = HashSet::new();
            for tok in &ref_author_tokens {
                let Some(indices) = self.by_surname.get(tok) else {
                    continue;
                };
                for &idx in indices {
                    if !seen_docs.insert(idx) {
                        continue;
                    }
                    let doc = &self.docs[idx];
                    if !part_markers_compatible(doc.part_marker, ref_part) {
                        continue;
                    }
                    // Contiguous subslice in either direction (e.g. "algebraic geometry" inside
                    // "r hartshorne algebraic geometry gtm 52 springer 1977", or "functional analysis"
                    // inside "functional analysis an introduction").
                    if doc.words.len() >= 2
                        && total_words_chars(&doc.words) >= 9
                        && (contains_contiguous_subslice(&full_ref_words, &doc.words)
                            || contains_contiguous_subslice(&ref_words, &doc.words))
                    {
                        return true;
                    }
                    if ref_words.len() >= 2
                        && total_words_chars(&ref_words) >= 9
                        && contains_contiguous_subslice(&doc.words, &ref_words)
                    {
                        return true;
                    }
                    // Token overlap for slightly reworded / abbreviated titles by the same author
                    if doc.sig_words.len() >= 2 && ref_sig.len() >= 2 {
                        let inter = doc.sig_words.intersection(&ref_sig).count();
                        let union = doc.sig_words.len() + ref_sig.len() - inter;
                        let min_len = doc.sig_words.len().min(ref_sig.len());
                        if inter >= 2
                            && ((inter * 100 >= union * 68)
                                || (inter * 100 >= min_len * 85 && min_len >= 2))
                        {
                            return true;
                        }
                    }
                }
            }
        }

        // 5. High title-token overlap (catches missing-author citations and unenriched heuristic files
        //    such as `Evans - Partial Differential Equations.pdf`).
        let full_ref_sig = significant_word_set(&full_ref_words);
        let mut cand_hits: HashMap<usize, usize> = HashMap::new();
        let mut query_words: Vec<&String> = full_ref_sig.iter().filter(|w| w.len() >= 4).collect();
        query_words.sort_by_key(|w| self.by_word.get(*w).map_or(0, Vec::len));
        for w in query_words.into_iter().take(8) {
            if let Some(indices) = self.by_word.get(w) {
                if indices.len() > 250 {
                    continue;
                }
                for &idx in indices {
                    *cand_hits.entry(idx).or_insert(0) += 1;
                }
            }
        }

        for (idx, shared_rare) in cand_hits {
            if shared_rare < 2 {
                continue;
            }
            let doc = &self.docs[idx];
            if !part_markers_compatible(doc.part_marker, ref_part) {
                continue;
            }

            // Unenriched local file whose filename/title contains both the cited author surname and title
            if doc.surnames.is_empty() {
                if !ref_author_tokens.is_empty()
                    && ref_author_tokens.iter().any(|s| doc.heuristic_word_set.contains(s))
                    && ref_words.len() >= 2
                    && total_words_chars(&ref_words) >= 9
                {
                    if contains_contiguous_subslice(&doc.heuristic_words, &ref_words) {
                        return true;
                    }
                    let inter = ref_sig
                        .iter()
                        .filter(|w| doc.heuristic_word_set.contains(*w))
                        .count();
                    if inter >= 2 && inter * 100 >= ref_sig.len() * 85 {
                        return true;
                    }
                }
                // Also match Crossref unstructured strings directly against heuristic title/filename words
                if doc.words.len() >= 3
                    && total_words_chars(&doc.words) >= 14
                    && contains_contiguous_subslice(&full_ref_words, &doc.words)
                {
                    return true;
                }
                if doc.sig_words.len() >= 3 {
                    let inter = doc.sig_words.intersection(&full_ref_sig).count();
                    if inter >= 3 && inter * 100 >= doc.sig_words.len() * 85 {
                        return true;
                    }
                }
            }

            // Very strong multi-word title similarity even without matching author fields
            if doc.sig_words.len() >= 3 && ref_sig.len() >= 3 {
                let inter = doc.sig_words.intersection(&ref_sig).count();
                let union = doc.sig_words.len() + ref_sig.len() - inter;
                let min_len = doc.sig_words.len().min(ref_sig.len());
                let author_conflict = !ref_author_tokens.is_empty()
                    && !doc.surnames.is_empty()
                    && !doc.surnames.iter().any(|s| ref_author_tokens.contains(s));
                if !author_conflict
                    && ((inter >= 3 && inter * 100 >= union * 82)
                        || (inter >= 4 && inter * 100 >= min_len * 90))
                {
                    return true;
                }
            }
        }

        false
    }
}

/// Aggregates the most-cited works referenced by documents in the user's library (via already-cached
/// Crossref and Semantic Scholar payloads, zero extra network calls) that are NOT yet owned locally.
/// `category`: `None` / `"all"` | `"paper"` | `"book"`.
pub fn find_missing_cited_classics(
    conn: &Connection,
    category: Option<&str>,
    limit: usize,
) -> Result<Vec<MissingClassicItem>> {
    let cap = limit.clamp(1, 100);
    let cat_filter = match category.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("paper") => Some("paper"),
        Some("book") => Some("book"),
        _ => None,
    };

    let OwnedMatcherBuild {
        matcher: owned_matcher,
        doc_by_doi,
        doc_by_arxiv,
        citer_meta,
    } = OwnedDocMatcher::from_db(conn)?;

    if doc_by_doi.is_empty() && doc_by_arxiv.is_empty() {
        return Ok(Vec::new());
    }

    let mut agg_map: HashMap<String, MissingRefAgg> = HashMap::new();

    let mut record_ref = |citing_doc_id: i64,
                          title_raw: &str,
                          authors_raw: &str,
                          unstructured_raw: Option<&str>,
                          year: Option<i64>,
                          venue: Option<String>,
                          doi: Option<String>,
                          arxiv_id: Option<String>,
                          isbn: Option<String>,
                          mr: Option<String>,
                          is_book: bool| {
        let title = clean_ref_title(title_raw);
        if title.len() < 4 {
            return;
        }
        let check = RefCheckInput {
            title: &title,
            authors: authors_raw,
            unstructured: unstructured_raw,
            year,
            doi: doi.as_deref(),
            arxiv_id: arxiv_id.as_deref(),
            isbn: isbn.as_deref(),
            mr: mr.as_deref(),
        };
        if owned_matcher.is_owned(&check) {
            return;
        }
        let title_norm = clean_canonical_words(&tokenize_alnum_words(&title)).join(" ");
        if title_norm.len() < 4 {
            return;
        }

        let key = if let Some(ref d) = doi {
            format!("doi:{d}")
        } else if let Some(ref ib) = isbn {
            if let Some(core) = isbn9_core(ib) {
                format!("isbn9:{core}")
            } else {
                format!("isbn:{ib}")
            }
        } else if let Some(ref ax) = arxiv_id {
            format!("arxiv:{ax}")
        } else {
            let short: String = title_norm.chars().take(72).collect();
            format!("t:{short}:{}", year.unwrap_or(0))
        };

        let entry = agg_map.entry(key).or_default();
        if entry.title.len() < title.len() {
            entry.title = title;
        }
        let au = authors_raw.trim();
        if entry.authors.len() < au.len() {
            entry.authors = au.to_string();
        }
        if entry.year.is_none() {
            entry.year = year;
        }
        if entry.venue.is_none() {
            entry.venue = venue;
        }
        if entry.doi.is_none() {
            entry.doi = doi;
        }
        if entry.arxiv_id.is_none() {
            entry.arxiv_id = arxiv_id;
        }
        if entry.isbn.is_none() {
            entry.isbn = isbn;
        }
        if is_book {
            entry.is_book_votes += 1;
        } else {
            entry.is_paper_votes += 1;
        }
        entry.citers.insert(citing_doc_id);
    };

    // Scan cached Crossref & Semantic Scholar payloads matching library docs.
    let mut stmt = conn.prepare(
        "SELECT source, lookup_key, response_body
         FROM metadata_http_cache
         WHERE source IN ('crossref', 'semanticscholar')
           AND status_code < 400",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;

    for row in rows.flatten() {
        let (source, lookup_key, body) = row;
        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            continue;
        };

        if source == "crossref" {
            // Collect `(citing_doc_id, reference_array)` from BOTH direct `doi:` payloads (`message.reference`)
            // and title-search list payloads (`message.items[].reference` matched by item DOI).
            let mut cr_batches: Vec<(i64, &[Value])> = Vec::new();
            if let Some(d) = lookup_key.strip_prefix("doi:").and_then(normalize_doi) {
                if let Some(&cid) = doc_by_doi.get(&d) {
                    if let Some(refs) = v
                        .get("message")
                        .and_then(|m| m.get("reference"))
                        .and_then(Value::as_array)
                    {
                        cr_batches.push((cid, refs.as_slice()));
                    }
                }
            }
            if let Some(items) = v
                .get("message")
                .and_then(|m| m.get("items"))
                .and_then(Value::as_array)
            {
                for item in items {
                    let Some(d) = item.get("DOI").and_then(Value::as_str).and_then(normalize_doi) else {
                        continue;
                    };
                    let Some(&cid) = doc_by_doi.get(&d) else {
                        continue;
                    };
                    if let Some(refs) = item.get("reference").and_then(Value::as_array) {
                        cr_batches.push((cid, refs.as_slice()));
                    }
                }
            }

            for (citing_id, refs) in cr_batches {
                for r in refs {
                    let unstructured = r
                        .get("unstructured")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    let doi = r
                        .get("DOI")
                        .and_then(Value::as_str)
                        .and_then(normalize_doi)
                        .or_else(|| {
                            unstructured.and_then(|u| {
                                ref_doi_re().find(u).and_then(|m| normalize_doi(m.as_str()))
                            })
                        });
                    let arxiv_id = unstructured.and_then(|u| {
                        ref_arxiv_re()
                            .captures(u)
                            .and_then(|c| c.get(1))
                            .and_then(|m| normalize_arxiv_base_id(m.as_str()))
                    });
                    let mr = unstructured.and_then(|u| {
                        ref_mr_re()
                            .captures(u)
                            .and_then(|c| c.get(1))
                            .and_then(|m| normalize_mr_digits(m.as_str()))
                    });
                    let isbn = r
                        .get("ISBN")
                        .and_then(Value::as_str)
                        .and_then(normalize_isbn_digits);
                    let article_title = r
                        .get("article-title")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    let volume_title = r
                        .get("volume-title")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    let journal_title = r
                        .get("journal-title")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    let author = r.get("author").and_then(Value::as_str).unwrap_or("").trim();
                    let year = r
                        .get("year")
                        .and_then(Value::as_str)
                        .and_then(extract_year_from_str)
                        .or_else(|| r.get("year").and_then(Value::as_i64));

                    let title_owned: String = if let Some(t) = article_title.or(volume_title) {
                        t.to_string()
                    } else if let Some(u) = unstructured {
                        u.chars().take(140).collect()
                    } else {
                        continue;
                    };
                    let venue = journal_title
                        .or(if article_title.is_some() { volume_title } else { None })
                        .or_else(|| r.get("series-title").and_then(Value::as_str))
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToOwned::to_owned);
                    let is_book = looks_like_book_reference(
                        isbn.is_some(),
                        article_title.is_some(),
                        volume_title,
                        journal_title,
                        venue.as_deref(),
                        &title_owned,
                    );
                    let eff_year = year.or_else(|| unstructured.and_then(extract_year_from_str));
                    record_ref(
                        citing_id,
                        &title_owned,
                        author,
                        unstructured,
                        eff_year,
                        venue,
                        doi,
                        arxiv_id,
                        isbn,
                        mr,
                        is_book,
                    );
                }
            }
        } else if source == "semanticscholar" {
            let citing_doc_id = if let Some(d) = lookup_key.strip_prefix("doi:").and_then(normalize_doi) {
                doc_by_doi.get(&d).copied()
            } else if let Some(ax) = lookup_key
                .strip_prefix("arxiv:")
                .and_then(normalize_arxiv_base_id)
            {
                doc_by_arxiv.get(&ax).copied()
            } else {
                None
            };
            let Some(citing_id) = citing_doc_id else {
                continue;
            };
            let Some(refs) = v.get("references").and_then(Value::as_array) else {
                continue;
            };
            for r in refs {
                let Some(title) = r
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let ext = r.get("externalIds");
                let doi = ext
                    .and_then(|e| e.get("DOI"))
                    .and_then(Value::as_str)
                    .and_then(normalize_doi);
                let arxiv_id = ext
                    .and_then(|e| e.get("ArXiv"))
                    .and_then(Value::as_str)
                    .and_then(normalize_arxiv_base_id);
                let year = r.get("year").and_then(Value::as_i64);
                let venue = r
                    .get("venue")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned);
                let authors = r
                    .get("authors")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|a| a.get("name").and_then(Value::as_str))
                            .take(4)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                let pub_types_book = r
                    .get("publicationTypes")
                    .and_then(Value::as_array)
                    .is_some_and(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .any(|t| t.eq_ignore_ascii_case("Book"))
                    });
                let is_book = pub_types_book
                    || looks_like_book_reference(
                        false,
                        true,
                        None,
                        venue.as_deref(),
                        venue.as_deref(),
                        title,
                    );
                record_ref(
                    citing_id,
                    title,
                    &authors,
                    None,
                    year,
                    venue,
                    doi,
                    arxiv_id,
                    None,
                    None,
                    is_book,
                );
            }
        }
    }

    let mut items: Vec<MissingClassicItem> = agg_map
        .into_values()
        .filter_map(|agg| {
            // Re-check aggregated record against library (in case author/title/IDs were split across citations)
            let final_check = RefCheckInput {
                title: &agg.title,
                authors: &agg.authors,
                unstructured: None,
                year: agg.year,
                doi: agg.doi.as_deref(),
                arxiv_id: agg.arxiv_id.as_deref(),
                isbn: agg.isbn.as_deref(),
                mr: None,
            };
            if owned_matcher.is_owned(&final_check) {
                return None;
            }
            let work_category = if agg.is_book_votes > agg.is_paper_votes {
                "book"
            } else {
                "paper"
            };
            if let Some(want) = cat_filter {
                if work_category != want {
                    return None;
                }
            }
            let mut citing_docs: Vec<MissingClassicCiter> = agg
                .citers
                .iter()
                .filter_map(|did| {
                    citer_meta.get(did).map(|(title, year)| MissingClassicCiter {
                        document_id: *did,
                        title: title.clone(),
                        year: *year,
                    })
                })
                .collect();
            citing_docs.sort_by(|a, b| a.title.cmp(&b.title));
            let cited_by_count = citing_docs.len();
            if cited_by_count == 0 {
                return None;
            }
            Some(MissingClassicItem {
                title: agg.title,
                authors: agg.authors,
                year: agg.year,
                venue: agg.venue,
                work_category: work_category.to_string(),
                doi: agg.doi,
                arxiv_id: agg.arxiv_id,
                isbn: agg.isbn,
                cited_by_count,
                citing_docs,
            })
        })
        .collect();

    items.sort_by(|a, b| {
        b.cited_by_count
            .cmp(&a.cited_by_count)
            .then_with(|| a.year.unwrap_or(9999).cmp(&b.year.unwrap_or(9999)))
            .then_with(|| a.title.cmp(&b.title))
    });
    items.truncate(cap);
    Ok(items)
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
    fn finds_similar_docs_local_citations_and_preprint_published_notice() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("rel.db"), 2).unwrap();
        let vindex = VectorIndex::open(&dir.path().join("rel.usearch"), 4).unwrap();
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: "r1".into(),
                label: "Math".into(),
                root_path: dir.path().to_string_lossy().into_owned(),
                is_network_or_cloud: false,
            },
        )
        .unwrap();

        let d1 = create_heuristic_document(&w, "Optimal Transport Paper").unwrap();
        w.execute(
            "UPDATE documents SET arxiv_id = '2301.12345' WHERE id = ?1",
            params![d1],
        )
        .unwrap();
        let f1 = insert_file_record(
            &w,
            &NewFileRecord {
                document_id: d1,
                root_id: "r1",
                rel_path: "ot.pdf",
                format: "pdf",
                size_bytes: 100,
                mtime_ns: 1,
                blake3_hash: "h1",
                index_state: "ready",
            },
        )
        .unwrap();
        let p1 = replace_file_pages_and_passages(
            &w,
            f1,
            1,
            true,
            None,
            &[FilePageInsert {
                file_id: f1,
                page_ord: 1,
                printed_label: Some("1".into()),
                width_pts: 600.0,
                height_pts: 800.0,
                text_source: "test".into(),
            }],
            &[PassageInsert {
                document_id: d1,
                file_id: f1,
                page_ord: 1,
                printed_label: Some("1".into()),
                chunk_index: 0,
                is_alt_formula: false,
                token_count: 15,
                text_content: "References: [1] See DOI 10.1007/s00222-020-00999-1 for Wasserstein bounds."
                    .into(),
                bboxes_blob: Vec::new(),
            }],
        )
        .unwrap();

        let d2 = create_heuristic_document(&w, "Wasserstein Bounds").unwrap();
        w.execute(
            "UPDATE documents SET doi = '10.1007/s00222-020-00999-1' WHERE id = ?1",
            params![d2],
        )
        .unwrap();
        let f2 = insert_file_record(
            &w,
            &NewFileRecord {
                document_id: d2,
                root_id: "r1",
                rel_path: "wass.pdf",
                format: "pdf",
                size_bytes: 100,
                mtime_ns: 1,
                blake3_hash: "h2",
                index_state: "ready",
            },
        )
        .unwrap();
        let p2 = replace_file_pages_and_passages(
            &w,
            f2,
            1,
            true,
            None,
            &[FilePageInsert {
                file_id: f2,
                page_ord: 1,
                printed_label: Some("1".into()),
                width_pts: 600.0,
                height_pts: 800.0,
                text_source: "test".into(),
            }],
            &[PassageInsert {
                document_id: d2,
                file_id: f2,
                page_ord: 1,
                printed_label: Some("1".into()),
                chunk_index: 0,
                is_alt_formula: false,
                token_count: 10,
                text_content: "Wasserstein gradient flow".into(),
                bboxes_blob: Vec::new(),
            }],
        )
        .unwrap();

        // Seed cached arXiv response carrying `<arxiv:doi>`
        w.execute(
            "INSERT INTO metadata_http_cache (source, lookup_key, response_body, status_code, fetched_at)
             VALUES ('arxiv', 'id:2301.12345', '<entry><arxiv:doi>10.1090/jams/999</arxiv:doi><arxiv:journal_ref>J. Amer. Math. Soc. 2024</arxiv:journal_ref></entry>', 200, 1)",
            [],
        )
        .unwrap();

        vindex
            .upsert_batch(&[
                (p1[0], vec![0.95, 0.31, 0.0, 0.0]),
                (p2[0], vec![0.93, 0.36, 0.0, 0.0]),
            ])
            .unwrap();

        let added = refresh_local_citations_for_doc(&w, d1).unwrap();
        assert_eq!(added, 1);

        let rel1 = get_document_relations(&w, Some(&vindex), d1, 5).unwrap();
        assert_eq!(rel1.similar.len(), 1);
        assert_eq!(rel1.similar[0].document_id, d2);
        assert_eq!(rel1.cites.len(), 1);
        assert_eq!(rel1.cites[0].document_id, d2);
        assert_eq!(
            rel1.published_notice.as_ref().and_then(|n| n.doi.as_deref()),
            Some("10.1090/jams/999")
        );

        let rel2 = get_document_relations(&w, Some(&vindex), d2, 5).unwrap();
        assert_eq!(rel2.cited_by.len(), 1);
        assert_eq!(rel2.cited_by[0].document_id, d1);
    }

    #[test]
    fn finds_missing_cited_classics_filtered_by_paper_and_book() {
        let db = Database::in_memory().unwrap();
        let w = db.writer();
        let d1 = create_heuristic_document(&w, "My Paper A").unwrap();
        let d2 = create_heuristic_document(&w, "My Paper B").unwrap();
        let owned = create_heuristic_document(&w, "Already Owned Classic").unwrap();
        w.execute(
            "UPDATE documents SET doi = '10.1000/paper-a' WHERE id = ?1",
            params![d1],
        )
        .unwrap();
        w.execute(
            "UPDATE documents SET doi = '10.1000/paper-b' WHERE id = ?1",
            params![d2],
        )
        .unwrap();
        w.execute(
            "UPDATE documents SET doi = '10.1000/owned' WHERE id = ?1",
            params![owned],
        )
        .unwrap();

        let cr_body = r#"{
            "message": {
                "reference": [
                    {"DOI": "10.1000/owned", "article-title": "Already Owned Classic", "year": "1990"},
                    {"DOI": "10.1000/classic-paper", "article-title": "On Spectral Operators", "author": "Kato", "journal-title": "Ann. of Math.", "year": "1966"},
                    {"ISBN": "9780387900513", "volume-title": "Functional Analysis", "author": "Rudin", "year": "1973"}
                ]
            }
        }"#;
        w.execute(
            "INSERT INTO metadata_http_cache (source, lookup_key, response_body, status_code, fetched_at)
             VALUES ('crossref', 'doi:10.1000/paper-a', ?1, 200, 1)",
            params![cr_body],
        )
        .unwrap();
        w.execute(
            "INSERT INTO metadata_http_cache (source, lookup_key, response_body, status_code, fetched_at)
             VALUES ('crossref', 'doi:10.1000/paper-b', ?1, 200, 1)",
            params![cr_body],
        )
        .unwrap();

        let all = find_missing_cited_classics(&w, Some("all"), 20).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|c| c.cited_by_count == 2));
        assert!(all.iter().all(|c| c.doi.as_deref() != Some("10.1000/owned")));

        let books = find_missing_cited_classics(&w, Some("book"), 20).unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].title, "Functional Analysis");
        assert_eq!(books[0].work_category, "book");

        let papers = find_missing_cited_classics(&w, Some("paper"), 20).unwrap();
        assert_eq!(papers.len(), 1);
        assert_eq!(papers[0].title, "On Spectral Operators");
        assert_eq!(papers[0].work_category, "paper");
    }

    #[test]
    fn excludes_already_owned_books_and_papers_across_fuzzy_variants_without_false_positives() {
        let db = Database::in_memory().unwrap();
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: "r1".into(),
                label: "Math".into(),
                root_path: "/tmp/math".into(),
                is_network_or_cloud: false,
            },
        )
        .unwrap();

        let citer = create_heuristic_document(&w, "Citing Survey").unwrap();
        w.execute(
            "UPDATE documents SET doi = '10.1000/survey-1' WHERE id = ?1",
            params![citer],
        )
        .unwrap();

        // 1. Owned book with ISBN-10 and structured author (no DOI) -> cited with ISBN-13 & unstructured
        let hartshorne = create_heuristic_document(&w, "Algebraic Geometry").unwrap();
        w.execute(
            "UPDATE documents SET work_type = 'book', isbn = '0-387-90244-9', year = 1977 WHERE id = ?1",
            params![hartshorne],
        )
        .unwrap();
        w.execute(
            "INSERT INTO document_authors (document_id, role, position, family_name, given_name, name_norm)
             VALUES (?1, 'author', 0, 'Hartshorne', 'Robin', 'hartshorne')",
            params![hartshorne],
        )
        .unwrap();

        // 2. Owned book with subtitle (Brezis) -> should NOT exclude Rudin's Functional Analysis!
        let brezis = create_heuristic_document(
            &w,
            "Functional Analysis, Sobolev Spaces and Partial Differential Equations",
        )
        .unwrap();
        w.execute(
            "UPDATE documents SET work_type = 'book', year = 2011 WHERE id = ?1",
            params![brezis],
        )
        .unwrap();
        w.execute(
            "INSERT INTO document_authors (document_id, role, position, family_name, given_name, name_norm)
             VALUES (?1, 'author', 0, 'Brezis', 'Haim', 'brezis')",
            params![brezis],
        )
        .unwrap();

        // 3. Owned book with subtitle -> cited without subtitle by same author
        let folland = create_heuristic_document(&w, "Real Analysis: Modern Techniques and Their Applications").unwrap();
        w.execute(
            "UPDATE documents SET work_type = 'book', year = 1999 WHERE id = ?1",
            params![folland],
        )
        .unwrap();
        w.execute(
            "INSERT INTO document_authors (document_id, role, position, family_name, given_name, name_norm)
             VALUES (?1, 'author', 0, 'Folland', 'Gerald', 'folland')",
            params![folland],
        )
        .unwrap();

        // 4. Owned unenriched heuristic PDF (no document_authors, no DOI/ISBN)
        let evans = create_heuristic_document(&w, "Evans - Partial Differential Equations").unwrap();
        insert_file_record(
            &w,
            &NewFileRecord {
                document_id: evans,
                root_id: "r1",
                rel_path: "Books/Evans - Partial Differential Equations.pdf",
                format: "pdf",
                size_bytes: 100,
                mtime_ns: 1,
                blake3_hash: "h-evans",
                index_state: "ready",
            },
        )
        .unwrap();

        // 5. Owned book with DOI -> cited via chapter DOI suffix `_4`
        let kato = create_heuristic_document(&w, "Perturbation Theory for Linear Operators").unwrap();
        w.execute(
            "UPDATE documents SET work_type = 'book', doi = '10.1007/978-3-642-66282-9' WHERE id = ?1",
            params![kato],
        )
        .unwrap();

        // 6. Owned paper Part I -> Part II by same author MUST still be suggested!
        let part1 = create_heuristic_document(&w, "Regularity for Navier-Stokes Equations I").unwrap();
        w.execute(
            "INSERT INTO document_authors (document_id, role, position, family_name, given_name, name_norm)
             VALUES (?1, 'author', 0, 'Smith', 'Alice', 'smith')",
            params![part1],
        )
        .unwrap();

        // 7. Owned paper with minor typographic variant (en-dash vs hyphen, trailing period)
        let ckn = create_heuristic_document(
            &w,
            "Partial regularity of suitable weak solutions of the Navier-Stokes equations",
        )
        .unwrap();
        w.execute(
            "INSERT INTO document_authors (document_id, role, position, family_name, given_name, name_norm)
             VALUES (?1, 'author', 0, 'Caffarelli', 'Luis', 'caffarelli')",
            params![ckn],
        )
        .unwrap();

        let cr_body = r#"{
            "message": {
                "reference": [
                    {"ISBN": "978-0-387-90244-9", "volume-title": "Algebraic Geometry", "author": "R. Hartshorne", "year": "1977"},
                    {"unstructured": "R. Hartshorne, Algebraic Geometry, Graduate Texts in Mathematics 52, Springer-Verlag, New York, 1977."},
                    {"volume-title": "Real Analysis", "author": "G. B. Folland", "year": "1999"},
                    {"volume-title": "Partial Differential Equations", "author": "L. C. Evans", "year": "1998"},
                    {"unstructured": "L. C. Evans, Partial Differential Equations, AMS, 1998."},
                    {"DOI": "10.1007/978-3-642-66282-9_4", "volume-title": "Chapter 4: Stability Theorems", "author": "T. Kato", "year": "1976"},
                    {"article-title": "Partial regularity of suitable weak solutions of the Navier–Stokes equations.", "author": "L. Caffarelli", "journal-title": "Comm. Pure Appl. Math.", "year": "1982"},
                    {"ISBN": "9780070542365", "volume-title": "Functional Analysis", "author": "W. Rudin", "year": "1991"},
                    {"DOI": "10.1000/smith-part2", "article-title": "Regularity for Navier-Stokes Equations II", "author": "A. Smith", "journal-title": "Invent. Math.", "year": "2022"}
                ]
            }
        }"#;
        w.execute(
            "INSERT INTO metadata_http_cache (source, lookup_key, response_body, status_code, fetched_at)
             VALUES ('crossref', 'doi:10.1000/survey-1', ?1, 200, 1)",
            params![cr_body],
        )
        .unwrap();

        let res = find_missing_cited_classics(&w, Some("all"), 20).unwrap();
        let titles: Vec<&str> = res.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Functional Analysis", "Regularity for Navier-Stokes Equations II"],
            "Expected only Rudin's Functional Analysis and Smith's Part II, got: {titles:?}"
        );
    }

    #[test]
    fn resolve_citation_snippet_matches_owned_identifiers_and_fuzzy_bib_entries() {
        use crate::db::repos::docs::NewFileRecord;
        use crate::db::repos::roots::{insert_source_root, NewSourceRoot};
        use crate::db::Database;

        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(&tmp.path().join("cite_bridge.sqlite"), 2).unwrap();
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: "r1".into(),
                label: "Papers".into(),
                root_path: "/tmp/papers".into(),
                is_network_or_cloud: false,
            },
        )
        .unwrap();

        let add_with_file = |title: &str, name: &str| -> i64 {
            let doc_id = create_heuristic_document(&w, title).unwrap();
            let rel = format!("{name}.pdf");
            let hash = format!("b3_{name}");
            insert_file_record(
                &w,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: "r1",
                    rel_path: &rel,
                    format: "pdf",
                    size_bytes: 1024,
                    mtime_ns: 1,
                    blake3_hash: &hash,
                    index_state: "ready",
                },
            )
            .unwrap();
            doc_id
        };

        let citing = add_with_file("Current Survey Paper", "survey");
        let hartshorne = add_with_file("Algebraic Geometry", "hartshorne");
        w.execute(
            "UPDATE documents SET work_type = 'book', year = 1977, isbn = '0-387-90244-9' WHERE id = ?1",
            params![hartshorne],
        )
        .unwrap();
        w.execute(
            "INSERT INTO document_authors (document_id, role, position, family_name, given_name, name_norm)
             VALUES (?1, 'author', 0, 'Hartshorne', 'Robin', 'hartshorne')",
            params![hartshorne],
        )
        .unwrap();

        let ckn = add_with_file(
            "Partial regularity of suitable weak solutions of the Navier-Stokes equations",
            "ckn82",
        );
        w.execute(
            "UPDATE documents SET year = 1982, doi = '10.1002/cpa.3160350604' WHERE id = ?1",
            params![ckn],
        )
        .unwrap();
        w.execute(
            "INSERT INTO document_authors (document_id, role, position, family_name, given_name, name_norm)
             VALUES (?1, 'author', 0, 'Caffarelli', 'Luis', 'caffarelli')",
            params![ckn],
        )
        .unwrap();

        let part1 = add_with_file("Regularity for Navier-Stokes Equations I", "part1");
        w.execute(
            "UPDATE documents SET year = 2021 WHERE id = ?1",
            params![part1],
        )
        .unwrap();
        w.execute(
            "INSERT INTO document_authors (document_id, role, position, family_name, given_name, name_norm)
             VALUES (?1, 'author', 0, 'Smith', 'Alice', 'smith')",
            params![part1],
        )
        .unwrap();

        // 1. Unstructured book entry without ISBN matches Hartshorne via title + author + year
        let m1 = resolve_citation_snippet_in_library(
            &w,
            Some(citing),
            "[Har77] R. Hartshorne, Algebraic Geometry, Graduate Texts in Mathematics 52, Springer, 1977.",
        )
        .unwrap()
        .expect("should match Hartshorne");
        assert_eq!(m1.document_id, hartshorne);

        // 2. Entry with typographic en-dash and no DOI still matches CKN82
        let m2 = resolve_citation_snippet_in_library(
            &w,
            Some(citing),
            "[CKN82] L. Caffarelli, R. Kohn, L. Nirenberg, Partial regularity of suitable weak solutions of the Navier–Stokes equations, Comm. Pure Appl. Math. 35 (1982), 771–831.",
        )
        .unwrap()
        .expect("should match CKN82");
        assert_eq!(m2.document_id, ckn);

        // 3. Exact DOI match
        let m3 = resolve_citation_snippet_in_library(
            &w,
            Some(citing),
            "[14] See https://doi.org/10.1002/cpa.3160350604 for details.",
        )
        .unwrap()
        .expect("should match CKN82 by DOI");
        assert_eq!(m3.document_id, ckn);
        assert_eq!(m3.source, "doi");

        // 4. Unowned Part II must NOT falsely match owned Part I
        let miss = resolve_citation_snippet_in_library(
            &w,
            Some(citing),
            "[Sm22] A. Smith, Regularity for Navier-Stokes Equations II, Invent. Math., 2022.",
        )
        .unwrap();
        assert!(miss.is_none(), "Part II should not match Part I: {miss:?}");
    }
}
