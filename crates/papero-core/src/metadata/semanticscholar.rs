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

const S2_PAPER_API: &str = "https://api.semanticscholar.org/graph/v1/paper";
const S2_FIELDS: &str =
    "title,authors,year,venue,publicationVenue,journal,externalIds,abstract,publicationTypes,url";
const S2_LICENSE_NOTE: &str = "Semantic Scholar Academic Graph (AI2)";

#[derive(Debug)]
pub struct SemanticScholarClient {
    http: Client,
    limiter: SequentialRateLimiter,
}

impl SemanticScholarClient {
    pub fn new(user_agent: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| CoreError::Network(format!("semanticscholar client init: {e}")))?;
        Ok(Self {
            http,
            limiter: SequentialRateLimiter::new(Duration::from_millis(1100)),
        })
    }

    pub async fn lookup(
        &self,
        db: &Database,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        if let Some((paper_key, cache_key)) = direct_paper_key(ids) {
            let maybe_body = if let Some(c) = load_http_cache(db, "semanticscholar", &cache_key)? {
                (c.status_code < 400).then_some(c.body)
            } else {
                self.limiter.acquire().await;
                let url = format!("{S2_PAPER_API}/{paper_key}?fields={S2_FIELDS}");
                if let Ok(resp) = self.http.get(&url).send().await {
                    let status = resp.status().as_u16();
                    if let Ok(text) = resp.text().await {
                        // Never cache transient 429 or 5xx responses from S2 public pool
                        if status < 400 || status == 404 {
                            store_http_cache(db, "semanticscholar", &cache_key, status, &text)?;
                        }
                        (status < 400).then_some(text)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(body) = maybe_body {
                if let Ok(v) = serde_json::from_str::<Value>(&body) {
                    if let Some(rec) = parse_s2_paper(&v) {
                        return Ok(vec![CandidateMatch {
                            score: 0.97,
                            source: "semanticscholar".to_string(),
                            record: rec,
                        }]);
                    }
                }
            }
        }

        let surname = ids
            .heuristic_first_author
            .as_deref()
            .and_then(clean_author_surname)
            .unwrap_or_default();

        let (query_str, year_param, cache_key) = if let Some(ti) = ids
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

            // Dedicated Semantic Scholar fuzzy title-match endpoint (`/paper/search/match`)
            if normalize_math_text(ti).split_whitespace().count() >= 3
                && load_http_cache(db, "semanticscholar", &ck)?.is_none()
            {
                if let Ok(match_hits) = self.lookup_title_match(db, ti, ids).await {
                    if match_hits.first().map(|c| c.score).unwrap_or(0.0) >= 0.68 {
                        return Ok(match_hits);
                    }
                }
            }

            (q, None, ck)
        } else if !surname.is_empty() && ids.heuristic_year.is_some() {
            let yr = ids.heuristic_year.unwrap_or(0);
            let kw = ids.filename_keywords.join(" ");
            let q = if kw.is_empty() {
                surname.clone()
            } else {
                format!("{surname} {kw}")
            };
            let yp = format!("{}-{}", yr - 1, yr + 1);
            let ck = format!("auyr:{yr}:{}", normalize_math_text(&q));
            (q, Some(yp), ck)
        } else {
            return Ok(Vec::new());
        };

        let body = if let Some(c) = load_http_cache(db, "semanticscholar", &cache_key)? {
            if c.status_code >= 400 {
                return Ok(Vec::new());
            }
            c.body
        } else {
            self.limiter.acquire().await;
            let search_endpoint = format!("{S2_PAPER_API}/search");
            let mut params = vec![
                ("query", query_str.as_str()),
                ("limit", "8"),
                ("fields", S2_FIELDS),
            ];
            if let Some(ref yp) = year_param {
                params.push(("year", yp.as_str()));
            }
            let req_url = url::Url::parse_with_params(&search_endpoint, &params)
                .map_err(|e| CoreError::Network(format!("semanticscholar url build: {e}")))?;

            let Ok(resp) = self.http.get(req_url).send().await else {
                return Ok(Vec::new());
            };
            let status = resp.status().as_u16();
            let Ok(text) = resp.text().await else {
                return Ok(Vec::new());
            };
            if status < 400 || status == 404 {
                store_http_cache(db, "semanticscholar", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        parse_s2_search_response(&body, ids)
    }

    async fn lookup_title_match(
        &self,
        db: &Database,
        title: &str,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        let cache_key = format!("match:{}", normalize_math_text(title));
        let body = if let Some(c) = load_http_cache(db, "semanticscholar", &cache_key)? {
            if c.status_code >= 400 {
                return Ok(Vec::new());
            }
            c.body
        } else {
            self.limiter.acquire().await;
            let match_endpoint = format!("{S2_PAPER_API}/search/match");
            let req_url = url::Url::parse_with_params(
                &match_endpoint,
                &[("query", title), ("fields", S2_FIELDS)],
            )
            .map_err(|e| CoreError::Network(format!("semanticscholar match url: {e}")))?;
            let Ok(resp) = self.http.get(req_url).send().await else {
                return Ok(Vec::new());
            };
            let status = resp.status().as_u16();
            let Ok(text) = resp.text().await else {
                return Ok(Vec::new());
            };
            if status < 400 || status == 404 {
                store_http_cache(db, "semanticscholar", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        parse_s2_search_response(&body, ids)
    }
}

fn direct_paper_key(ids: &ExtractedIdentifiers) -> Option<(String, String)> {
    if let Some(doi) = ids.doi.as_deref() {
        let encoded: String = form_urlencoded::byte_serialize(doi.as_bytes()).collect();
        return Some((format!("DOI:{encoded}"), format!("doi:{doi}")));
    }
    if let Some(ax) = ids.arxiv_id.as_deref() {
        let encoded: String = form_urlencoded::byte_serialize(ax.as_bytes()).collect();
        return Some((format!("ARXIV:{encoded}"), format!("arxiv:{ax}")));
    }
    None
}

pub fn parse_s2_search_response(
    body: &str,
    ids: &ExtractedIdentifiers,
) -> Result<Vec<CandidateMatch>> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| CoreError::Network(format!("semanticscholar json: {e}")))?;
    let mut out = Vec::new();
    for item in v
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(rec) = parse_s2_paper(item) {
            let score = score_candidate(&rec, ids);
            out.push(CandidateMatch {
                score,
                source: "semanticscholar".to_string(),
                record: rec,
            });
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

pub fn parse_s2_paper(p: &Value) -> Option<BibliographicRecord> {
    let raw_title = p.get("title").and_then(Value::as_str)?;
    let title = clean_metadata_text(raw_title);
    if title.is_empty() {
        return None;
    }

    let mut authors = Vec::new();
    for a in p
        .get("authors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(60)
    {
        if let Some(name) = a.get("name").and_then(Value::as_str) {
            if let Some(parsed) = split_full_author_name(name, "author") {
                authors.push(parsed);
            }
        }
    }

    let pub_types: Vec<&str> = p
        .get("publicationTypes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();

    let pub_venue = p.get("publicationVenue");
    let venue_type = pub_venue
        .and_then(|v| v.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let work_type = if pub_types.iter().any(|t| t.eq_ignore_ascii_case("Conference"))
        || venue_type.eq_ignore_ascii_case("conference")
    {
        WorkType::Inproceedings
    } else if pub_types.iter().any(|t| t.eq_ignore_ascii_case("Book")) {
        WorkType::Book
    } else {
        WorkType::Article
    };

    let journal_obj = p.get("journal");
    let venue_name = journal_obj
        .and_then(|j| j.get("name"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            pub_venue
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| {
            p.get("venue")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        })
        .map(clean_metadata_text);

    let (journal_full, booktitle) = match work_type {
        WorkType::Inproceedings | WorkType::Incollection => (None, venue_name),
        _ => (venue_name, None),
    };

    let volume = journal_obj
        .and_then(|j| j.get("volume"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let pages = journal_obj
        .and_then(|j| j.get("pages"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(normalize_pages);

    let issn = pub_venue
        .and_then(|v| v.get("issn"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let ext = p.get("externalIds");
    let doi = ext
        .and_then(|e| e.get("DOI"))
        .and_then(Value::as_str)
        .and_then(normalize_doi);
    let arxiv_id = ext
        .and_then(|e| e.get("ArXiv"))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let year = p.get("year").and_then(Value::as_i64);
    let abstract_text = p
        .get("abstract")
        .and_then(Value::as_str)
        .map(clean_metadata_text)
        .filter(|s| !s.is_empty());

    let url = doi
        .as_ref()
        .map(|d| format!("https://doi.org/{d}"))
        .or_else(|| p.get("url").and_then(Value::as_str).map(ToOwned::to_owned));

    Some(BibliographicRecord {
        work_type,
        title,
        authors,
        journal_full,
        booktitle,
        volume,
        year,
        pages,
        issn,
        doi,
        arxiv_id,
        url,
        abstract_text,
        source: "semanticscholar".to_string(),
        license_note: Some(S2_LICENSE_NOTE.to_string()),
        ..BibliographicRecord::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semantic_scholar_conference_paper() {
        let sample = r#"{
          "data": [
            {
              "title": "Physics-informed neural networks: A deep learning framework for solving forward and inverse problems involving nonlinear partial differential equations",
              "year": 2019,
              "venue": "Journal of Computational Physics",
              "publicationTypes": ["JournalArticle"],
              "authors": [
                { "name": "M. Raissi" },
                { "name": "P. Perdikaris" },
                { "name": "G. E. Karniadakis" }
              ],
              "journal": {
                "name": "Journal of Computational Physics",
                "volume": "378",
                "pages": "686-707"
              },
              "externalIds": {
                "DOI": "10.1016/j.jcp.2018.10.045"
              },
              "abstract": "We introduce physics-informed neural networks.",
              "url": "https://www.semanticscholar.org/paper/abc"
            }
          ]
        }"#;

        let ids = ExtractedIdentifiers {
            doi: Some("10.1016/j.jcp.2018.10.045".to_string()),
            ..ExtractedIdentifiers::default()
        };
        let hits = parse_s2_search_response(sample, &ids).expect("parse s2");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].score >= 0.98);
        assert_eq!(hits[0].record.authors.len(), 3);
        assert_eq!(hits[0].record.authors[2].family_name, "Karniadakis");
        assert_eq!(hits[0].record.pages.as_deref(), Some("686--707"));
        assert_eq!(hits[0].record.volume.as_deref(), Some("378"));
    }
}
