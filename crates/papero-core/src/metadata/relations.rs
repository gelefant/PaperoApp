use crate::error::Result;
use crate::index::vector::VectorIndex;
use crate::metadata::extract::extract_doi_from_text;
use crate::metadata::models::{normalize_arxiv_base_id, normalize_doi, normalize_mr_digits};
use crate::ml::embedder::l2_normalize;
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

/// Scans bibliography passages + cached OpenAlex/Crossref/Semantic Scholar HTTP payloads for
/// `document_id` and records local outgoing citations in `document_citations`.
pub fn refresh_local_citations_for_doc(conn: &Connection, document_id: i64) -> Result<usize> {
    let lut = load_library_id_lookup(conn)?;
    if lut.by_doi.is_empty() && lut.by_arxiv.is_empty() && lut.by_mr.is_empty() {
        return Ok(0);
    }

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
        return Ok(0);
    };

    let mut edges: Vec<(i64, &'static str)> = Vec::new();
    let mut seen_cited: HashSet<i64> = HashSet::new();

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
                                edges.push((cited_id, "crossref"));
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
                                edges.push((cited_id, "semanticscholar"));
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
                                edges.push((cited_id, "semanticscholar"));
                            }
                        }
                    }
                }
            }
        } else if src == "openalex" {
            // OpenAlex stores `referenced_works: ["https://openalex.org/W..."]`.
            // Cross-reference against any cached OpenAlex work IDs belonging to local library DOIs.
            if let Some(ref_works) = v.get("referenced_works").and_then(Value::as_array) {
                let ref_set: HashSet<&str> =
                    ref_works.iter().filter_map(Value::as_str).collect();
                if !ref_set.is_empty() {
                    let mut c_stmt = conn.prepare(
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
                        if let Ok(other_v) = serde_json::from_str::<Value>(&rb) {
                            if other_v
                                .get("id")
                                .and_then(Value::as_str)
                                .is_some_and(|wid| ref_set.contains(wid))
                            {
                                seen_cited.insert(cited_id);
                                edges.push((cited_id, "openalex"));
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
    let mut p_stmt = conn.prepare(
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
                        edges.push((cited_id, "extracted_ref"));
                    }
                }
            }
        }
        for cap in ref_arxiv_re().captures_iter(&txt) {
            if let Some(ax) = cap.get(1).and_then(|m| normalize_arxiv_base_id(m.as_str())) {
                if let Some(&cited_id) = lut.by_arxiv.get(&ax) {
                    if cited_id != document_id && seen_cited.insert(cited_id) {
                        edges.push((cited_id, "extracted_ref"));
                    }
                }
            }
        }
        for cap in ref_mr_re().captures_iter(&txt) {
            if let Some(mr) = cap.get(1).and_then(|m| normalize_mr_digits(m.as_str())) {
                if let Some(&cited_id) = lut.by_mr.get(&mr) {
                    if cited_id != document_id && seen_cited.insert(cited_id) {
                        edges.push((cited_id, "extracted_ref"));
                    }
                }
            }
        }
    }

    if !edges.is_empty() {
        let mut ins = conn.prepare_cached(
            "INSERT OR IGNORE INTO document_citations (citing_doc_id, cited_doc_id, source, confidence)
             VALUES (?1, ?2, ?3, 1.0)",
        )?;
        for (cited_id, src) in &edges {
            ins.execute(params![document_id, cited_id, src])?;
        }
    }

    Ok(edges.len())
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
}
