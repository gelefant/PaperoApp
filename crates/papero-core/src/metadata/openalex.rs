use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::metadata::limiter::{load_http_cache, store_http_cache, SequentialRateLimiter};
use crate::metadata::models::{
    clean_author_surname, clean_metadata_text, normalize_doi, normalize_pages,
    split_full_author_name, BibliographicRecord, CandidateMatch, ExtractedIdentifiers, WorkType,
};
use crate::metadata::mrlookup::score_candidate;
use crate::text_norm::normalize_math_text;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use url::form_urlencoded;

const OPENALEX_WORKS_API: &str = "https://api.openalex.org/works";
const OPENALEX_LICENSE_NOTE: &str = "CC0 1.0 (OpenAlex)";

#[derive(Debug)]
pub struct OpenAlexClient {
    http: Client,
    polite_email: Option<String>,
    limiter: SequentialRateLimiter,
}

impl OpenAlexClient {
    pub fn new(app_version: &str, polite_email: Option<String>) -> Result<Self> {
        let cleaned_email = polite_email
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty());
        let ua = match &cleaned_email {
            Some(mail) => format!("Papero/{app_version} (mailto:{mail})"),
            None => format!("Papero/{app_version}"),
        };
        let http = Client::builder()
            .user_agent(ua)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| CoreError::Network(format!("openalex client init: {e}")))?;

        let interval_ms = if cleaned_email.is_some() { 150 } else { 400 };
        Ok(Self {
            http,
            polite_email: cleaned_email,
            limiter: SequentialRateLimiter::new(Duration::from_millis(interval_ms)),
        })
    }

    pub async fn lookup(
        &self,
        db: &Database,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        if let Some(doi) = ids.doi.as_deref() {
            if let Some(hit) = self.lookup_exact_doi(db, doi).await? {
                return Ok(vec![hit]);
            }
        }

        let surname = ids
            .heuristic_first_author
            .as_deref()
            .and_then(clean_author_surname)
            .unwrap_or_default();

        if let Some(ti) = ids
            .heuristic_title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let q = if surname.is_empty() {
                ti.to_string()
            } else {
                format!("{ti} {surname}")
            };
            let ck = format!("q:{}", normalize_math_text(&q));

            // Precision title-field filter (`filter=title.search:...`) so Title-only and math-title
            // queries match specifically in work titles rather than 250M fulltext abstracts.
            let mut hits = Vec::new();
            let clean_ti_filter = sanitize_openalex_title_filter(ti);
            if clean_ti_filter.split_whitespace().count() >= 2
                && load_http_cache(db, "openalex", &ck)?.is_none()
            {
                let ts_ck = format!("ts:{}", normalize_math_text(&q));
                let filter_expr = format!("title.search:{clean_ti_filter}");
                let au_search = (!surname.is_empty()).then_some(surname.as_str());
                hits = self
                    .search_works(db, au_search, Some(&filter_expr), &ts_ck, ids)
                    .await?;
            }

            let mut top_score = hits.first().map(|c| c.score).unwrap_or(0.0);
            if top_score < 0.62 {
                let broad_hits = self.search_works(db, Some(&q), None, &ck, ids).await?;
                if broad_hits.first().map(|c| c.score).unwrap_or(0.0) > top_score {
                    hits = broad_hits;
                    top_score = hits.first().map(|c| c.score).unwrap_or(0.0);
                }
            }

            let ti_words = normalize_math_text(ti).split_whitespace().count();
            if !surname.is_empty() && ti_words >= 4 && top_score < 0.58 {
                let fb_ck = format!("q:{}", normalize_math_text(ti));
                let mut ids_no_au = ids.clone();
                ids_no_au.heuristic_first_author = None;
                ids_no_au.heuristic_authors.clear();
                let fb_filter = (!clean_ti_filter.is_empty())
                    .then(|| format!("title.search:{clean_ti_filter}"));
                let mut fb_hits = if let Some(ref f_expr) = fb_filter {
                    let ts_fb_ck = format!("ts:{}", normalize_math_text(ti));
                    self.search_works(db, None, Some(f_expr), &ts_fb_ck, &ids_no_au)
                        .await?
                } else {
                    self.search_works(db, Some(ti), None, &fb_ck, &ids_no_au)
                        .await?
                };
                for h in &mut fb_hits {
                    h.score *= 0.97;
                }
                if fb_hits.first().map(|c| c.score).unwrap_or(0.0) > top_score {
                    hits = fb_hits;
                }
            }
            return Ok(hits);
        }

        if !surname.is_empty() {
            let kw = ids.filename_keywords.join(" ");
            let q = if kw.is_empty() {
                surname.clone()
            } else {
                format!("{surname} {kw}")
            };
            let yf = ids
                .heuristic_year
                .map(|yr| format!("publication_year:{}-{}", yr - 1, yr + 1));
            let ck = format!(
                "auyr:{}:{}",
                ids.heuristic_year.unwrap_or(0),
                normalize_math_text(&q)
            );
            return self
                .search_works(db, Some(&q), yf.as_deref(), &ck, ids)
                .await;
        }

        Ok(Vec::new())
    }

    pub async fn lookup_exact_doi(
        &self,
        db: &Database,
        doi: &str,
    ) -> Result<Option<CandidateMatch>> {
        let cache_key = format!("doi:{doi}");
        let body = if let Some(c) = load_http_cache(db, "openalex", &cache_key)? {
            if c.status_code >= 400 {
                return Ok(None);
            }
            c.body
        } else {
            self.limiter.acquire().await;
            let doi_target = format!("https://doi.org/{doi}");
            let encoded: String = form_urlencoded::byte_serialize(doi_target.as_bytes()).collect();
            let url = if let Some(mail) = &self.polite_email {
                let m_enc: String = form_urlencoded::byte_serialize(mail.as_bytes()).collect();
                format!("{OPENALEX_WORKS_API}/{encoded}?mailto={m_enc}")
            } else {
                format!("{OPENALEX_WORKS_API}/{encoded}")
            };
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .map_err(|e| CoreError::Network(format!("openalex doi request: {e}")))?;
            let status = resp.status().as_u16();
            let text = resp
                .text()
                .await
                .map_err(|e| CoreError::Network(format!("openalex doi read: {e}")))?;
            if status < 400 || status == 404 {
                store_http_cache(db, "openalex", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(None);
            }
            text
        };

        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            return Ok(None);
        };
        if let Some(rec) = parse_openalex_work(&v) {
            return Ok(Some(CandidateMatch {
                score: 0.98,
                source: "openalex".to_string(),
                record: rec,
            }));
        }
        Ok(None)
    }

    async fn search_works(
        &self,
        db: &Database,
        query_str: Option<&str>,
        filter_expr: Option<&str>,
        cache_key: &str,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        let body = if let Some(c) = load_http_cache(db, "openalex", cache_key)? {
            if c.status_code >= 400 {
                return Ok(Vec::new());
            }
            c.body
        } else {
            self.limiter.acquire().await;
            let mut query_pairs = vec![("per-page", "12")];
            if let Some(q) = query_str.filter(|s| !s.is_empty()) {
                query_pairs.push(("search", q));
            }
            if let Some(yf) = filter_expr {
                query_pairs.push(("filter", yf));
            }
            if let Some(mail) = &self.polite_email {
                query_pairs.push(("mailto", mail.as_str()));
            }
            let req_url = url::Url::parse_with_params(OPENALEX_WORKS_API, &query_pairs)
                .map_err(|e| CoreError::Network(format!("openalex search url: {e}")))?;
            let resp = self
                .http
                .get(req_url)
                .send()
                .await
                .map_err(|e| CoreError::Network(format!("openalex search request: {e}")))?;
            let status = resp.status().as_u16();
            let text = resp
                .text()
                .await
                .map_err(|e| CoreError::Network(format!("openalex search read: {e}")))?;
            if status < 400 || status == 404 {
                store_http_cache(db, "openalex", cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        parse_openalex_search_response(&body, ids)
    }
}

fn sanitize_openalex_title_filter(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if matches!(c, ',' | ':' | ';' | '|' | '!' | '"' | '(' | ')' | '[' | ']' | '{' | '}')
            {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_openalex_search_response(
    body: &str,
    ids: &ExtractedIdentifiers,
) -> Result<Vec<CandidateMatch>> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| CoreError::Network(format!("openalex list json: {e}")))?;
    let mut out = Vec::new();
    if let Some(items) = v.get("results").and_then(Value::as_array) {
        for item in items {
            if let Some(rec) = parse_openalex_work(item) {
                let score = score_candidate(&rec, ids);
                out.push(CandidateMatch {
                    score,
                    source: "openalex".to_string(),
                    record: rec,
                });
            }
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

pub fn parse_openalex_work(w: &Value) -> Option<BibliographicRecord> {
    let raw_title = w
        .get("title")
        .and_then(Value::as_str)
        .or_else(|| w.get("display_name").and_then(Value::as_str))?;
    let title = clean_metadata_text(raw_title);
    if title.is_empty() {
        return None;
    }

    let oa_type = w.get("type").and_then(Value::as_str).unwrap_or("article");
    let source_obj = w
        .get("primary_location")
        .and_then(|pl| pl.get("source"));
    let source_type = source_obj
        .and_then(|s| s.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let work_type = match oa_type {
        "book" | "monograph" | "edited-book" => WorkType::Book,
        "book-chapter" => WorkType::Incollection,
        "proceedings-article" => WorkType::Inproceedings,
        "dissertation" => WorkType::Phdthesis,
        "preprint" => WorkType::Misc,
        _ if source_type == "conference" => WorkType::Inproceedings,
        _ => WorkType::Article,
    };

    let mut authors = Vec::new();
    for ship in w
        .get("authorships")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(60)
    {
        if let Some(name) = ship
            .get("author")
            .and_then(|a| a.get("display_name"))
            .and_then(Value::as_str)
            .or_else(|| ship.get("raw_author_name").and_then(Value::as_str))
        {
            if let Some(parsed) = split_full_author_name(name, "author") {
                authors.push(parsed);
            }
        }
    }

    let venue_name = source_obj
        .and_then(|s| s.get("display_name"))
        .and_then(Value::as_str)
        .map(clean_metadata_text)
        .filter(|s| !s.is_empty());

    let (journal_full, booktitle) = match work_type {
        WorkType::Inproceedings | WorkType::Incollection => (None, venue_name),
        _ => (venue_name, None),
    };

    let issn = source_obj
        .and_then(|s| {
            s.get("issn_l").and_then(Value::as_str).or_else(|| {
                s.get("issn")
                    .and_then(Value::as_array)?
                    .first()?
                    .as_str()
            })
        })
        .map(ToOwned::to_owned);

    let publisher = source_obj
        .and_then(|s| s.get("host_organization_name"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let biblio = w.get("biblio");
    let volume = biblio
        .and_then(|b| b.get("volume"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let issue_number = biblio
        .and_then(|b| b.get("issue"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let first_page = biblio
        .and_then(|b| b.get("first_page"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let last_page = biblio
        .and_then(|b| b.get("last_page"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let pages = match (first_page, last_page) {
        (Some(fp), Some(lp)) if fp != lp => Some(normalize_pages(&format!("{fp}--{lp}"))),
        (Some(fp), _) => Some(fp.to_string()),
        _ => None,
    };

    let year = w.get("publication_year").and_then(Value::as_i64);
    let doi = w.get("doi").and_then(Value::as_str).and_then(normalize_doi);

    let mut arxiv_id = None;
    if let Some(locs) = w.get("locations").and_then(Value::as_array) {
        for loc in locs {
            if let Some(lp) = loc.get("landing_page_url").and_then(Value::as_str) {
                if let Some(idx) = lp.find("arxiv.org/abs/") {
                    let ax = lp[idx + "arxiv.org/abs/".len()..]
                        .trim_end_matches('/')
                        .split(['?', '#'])
                        .next()
                        .unwrap_or("")
                        .trim();
                    if !ax.is_empty() {
                        arxiv_id = Some(ax.to_string());
                        break;
                    }
                }
            }
        }
    }

    let url = doi
        .as_ref()
        .map(|d| format!("https://doi.org/{d}"))
        .or_else(|| {
            w.get("primary_location")
                .and_then(|pl| pl.get("landing_page_url"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .or_else(|| w.get("id").and_then(Value::as_str).map(ToOwned::to_owned));

    let abstract_text = w
        .get("abstract_inverted_index")
        .and_then(reconstruct_inverted_abstract);

    Some(BibliographicRecord {
        work_type,
        title,
        authors,
        journal_full,
        booktitle,
        volume,
        issue_number,
        year,
        pages,
        publisher,
        issn,
        doi,
        arxiv_id,
        url,
        abstract_text,
        source: "openalex".to_string(),
        license_note: Some(OPENALEX_LICENSE_NOTE.to_string()),
        ..BibliographicRecord::default()
    })
}

fn reconstruct_inverted_abstract(inv: &Value) -> Option<String> {
    let obj = inv.as_object()?;
    if obj.is_empty() {
        return None;
    }
    let mut max_pos = 0usize;
    let mut has_any = false;
    for positions in obj.values() {
        if let Some(arr) = positions.as_array() {
            for p in arr {
                if let Some(idx) = p.as_u64() {
                    let u = idx as usize;
                    if u <= 10_000 {
                        max_pos = max_pos.max(u);
                        has_any = true;
                    }
                }
            }
        }
    }
    if !has_any {
        return None;
    }
    let mut words: Vec<&str> = vec![""; max_pos + 1];
    for (token, positions) in obj {
        if let Some(arr) = positions.as_array() {
            for p in arr {
                if let Some(idx) = p.as_u64() {
                    let u = idx as usize;
                    if u <= max_pos {
                        words[u] = token.as_str();
                    }
                }
            }
        }
    }
    let joined: String = words
        .into_iter()
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned = clean_metadata_text(&joined);
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openalex_work_and_reconstructs_inverted_abstract() {
        let sample = r#"{
          "id": "https://openalex.org/W2741809807",
          "doi": "https://doi.org/10.1007/s00211-021-01234-5",
          "title": "Discontinuous Galerkin Methods on Polytopic Meshes",
          "publication_year": 2021,
          "type": "article",
          "primary_location": {
            "landing_page_url": "https://doi.org/10.1007/s00211-021-01234-5",
            "source": {
              "display_name": "Numerische Mathematik",
              "issn_l": "0029-599X",
              "host_organization_name": "Springer Science+Business Media",
              "type": "journal"
            }
          },
          "authorships": [
            { "author": { "display_name": "Paola F. Antonietti" } },
            { "author": { "display_name": "Andrea Cangiani" } }
          ],
          "biblio": {
            "volume": "149",
            "issue": "2",
            "first_page": "345",
            "last_page": "382"
          },
          "abstract_inverted_index": {
            "We": [0],
            "analyze": [1],
            "high-order": [2],
            "DG": [3],
            "schemes.": [4]
          }
        }"#;

        let v: Value = serde_json::from_str(sample).expect("valid json");
        let rec = parse_openalex_work(&v).expect("parsed work");
        assert_eq!(
            rec.title,
            "Discontinuous Galerkin Methods on Polytopic Meshes"
        );
        assert_eq!(rec.authors.len(), 2);
        assert_eq!(rec.authors[0].family_name, "Antonietti");
        assert_eq!(rec.journal_full.as_deref(), Some("Numerische Mathematik"));
        assert_eq!(rec.pages.as_deref(), Some("345--382"));
        assert_eq!(rec.doi.as_deref(), Some("10.1007/s00211-021-01234-5"));
        assert_eq!(
            rec.abstract_text.as_deref(),
            Some("We analyze high-order DG schemes.")
        );
    }
}
