use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::metadata::limiter::{load_http_cache, store_http_cache, SequentialRateLimiter};
use crate::metadata::models::{
    clean_author_surname, clean_metadata_text, normalize_doi, normalize_pages,
    split_full_author_name, BibliographicAuthor, BibliographicRecord, CandidateMatch,
    ExtractedIdentifiers, WorkType,
};
use crate::metadata::mrlookup::score_candidate;
use crate::text_norm::normalize_math_text;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

const INSPIRE_API: &str = "https://inspirehep.net/api/literature";
const INSPIRE_LICENSE_NOTE: &str = "CC0 1.0 (INSPIRE-HEP)";

#[derive(Debug)]
pub struct InspireHepClient {
    http: Client,
    limiter: SequentialRateLimiter,
}

impl InspireHepClient {
    pub fn new(user_agent: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| CoreError::Network(format!("inspirehep client init: {e}")))?;
        Ok(Self {
            http,
            limiter: SequentialRateLimiter::new(Duration::from_millis(800)),
        })
    }

    pub async fn lookup(
        &self,
        db: &Database,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        if let Some(ax) = ids.arxiv_id.as_deref() {
            if let Some(body) = self
                .fetch_inspire_raw(db, &format!("arxiv:{ax}"), &format!("arxiv:{ax}"))
                .await?
            {
                let hits = parse_inspirehep_response(&body, ids)?;
                if !hits.is_empty() {
                    return Ok(hits);
                }
            }
        }
        if let Some(doi) = ids
            .doi
            .as_deref()
            .filter(|d| !d.starts_with("10.48550/"))
        {
            if let Some(body) = self
                .fetch_inspire_raw(db, &format!("doi:{doi}"), &format!("doi:{doi}"))
                .await?
            {
                let hits = parse_inspirehep_response(&body, ids)?;
                if !hits.is_empty() {
                    return Ok(hits);
                }
            }
        }

        let Some(ti) = ids
            .heuristic_title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            return Ok(Vec::new());
        };
        let clean_ti = ti.replace('"', " ");
        let query_str = if let Some(surname) = ids
            .heuristic_first_author
            .as_deref()
            .and_then(clean_author_surname)
        {
            format!("t \"{clean_ti}\" and a {surname}")
        } else {
            format!("t \"{clean_ti}\"")
        };
        let cache_key = format!("q:{}", normalize_math_text(&query_str));
        let Some(body) = self.fetch_inspire_raw(db, &query_str, &cache_key).await? else {
            return Ok(Vec::new());
        };

        parse_inspirehep_response(&body, ids)
    }

    async fn fetch_inspire_raw(
        &self,
        db: &Database,
        query_str: &str,
        cache_key: &str,
    ) -> Result<Option<String>> {
        if let Some(c) = load_http_cache(db, "inspirehep", cache_key)? {
            if c.status_code >= 400 {
                return Ok(None);
            }
            return Ok(Some(c.body));
        }
        self.limiter.acquire().await;
        let req_url = url::Url::parse_with_params(INSPIRE_API, &[("q", query_str), ("size", "6")])
            .map_err(|e| CoreError::Network(format!("inspirehep url build: {e}")))?;
        let resp = self
            .http
            .get(req_url)
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("inspirehep request: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| CoreError::Network(format!("inspirehep read: {e}")))?;
        if status < 400 || status == 404 {
            store_http_cache(db, "inspirehep", cache_key, status, &text)?;
        }
        if status >= 400 {
            return Ok(None);
        }
        Ok(Some(text))
    }
}

pub fn parse_inspirehep_response(
    body: &str,
    ids: &ExtractedIdentifiers,
) -> Result<Vec<CandidateMatch>> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| CoreError::Network(format!("inspirehep json: {e}")))?;
    let mut out = Vec::new();
    let hits = v
        .get("hits")
        .and_then(|h| h.get("hits"))
        .and_then(Value::as_array);
    for hit in hits.into_iter().flatten() {
        let Some(meta) = hit.get("metadata") else {
            continue;
        };
        if let Some(rec) = parse_inspire_metadata(meta, hit.get("id")) {
            let score = score_candidate(&rec, ids);
            out.push(CandidateMatch {
                score,
                source: "inspirehep".to_string(),
                record: rec,
            });
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

fn parse_inspire_metadata(meta: &Value, hit_id: Option<&Value>) -> Option<BibliographicRecord> {
    let title = meta
        .get("titles")
        .and_then(Value::as_array)
        .and_then(|arr| {
            arr.iter()
                .filter_map(|t| t.get("title").and_then(Value::as_str))
                .map(clean_metadata_text)
                .find(|s| !s.is_empty())
        })?;

    let doc_type = meta
        .get("document_type")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .unwrap_or("article");
    let work_type = match doc_type {
        "book" => WorkType::Book,
        "book chapter" => WorkType::Incollection,
        "conference paper" | "proceedings" => WorkType::Inproceedings,
        "thesis" => WorkType::Phdthesis,
        _ => WorkType::Article,
    };

    let mut authors = Vec::new();
    for a in meta
        .get("authors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(50)
    {
        let last = a
            .get("last_name")
            .and_then(Value::as_str)
            .map(clean_metadata_text)
            .unwrap_or_default();
        let first = a
            .get("first_name")
            .and_then(Value::as_str)
            .map(clean_metadata_text)
            .unwrap_or_default();
        if !last.is_empty() || !first.is_empty() {
            authors.push(BibliographicAuthor {
                family_name: last,
                given_name: first,
                role: "author".to_string(),
            });
        } else if let Some(full) = a.get("full_name").and_then(Value::as_str) {
            if let Some(parsed) = split_full_author_name(full, "author") {
                authors.push(parsed);
            }
        }
    }

    let doi = meta
        .get("dois")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|d| d.get("value"))
        .and_then(Value::as_str)
        .and_then(normalize_doi);

    let arxiv_id = meta
        .get("arxiv_eprints")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|e| e.get("value"))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let pub_info = meta
        .get("publication_info")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first());

    let j_title = pub_info
        .and_then(|p| p.get("journal_title"))
        .and_then(Value::as_str)
        .map(clean_metadata_text)
        .filter(|s| !s.is_empty());

    let (journal_abbrev, journal_full, booktitle) = match work_type {
        WorkType::Inproceedings | WorkType::Incollection => {
            let bt = pub_info
                .and_then(|p| p.get("conference_record").or_else(|| p.get("journal_title")))
                .and_then(Value::as_str)
                .map(clean_metadata_text)
                .or(j_title);
            (None, None, bt)
        }
        _ => (j_title.clone(), j_title, None),
    };

    let volume = pub_info
        .and_then(|p| p.get("journal_volume"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let issue_number = pub_info
        .and_then(|p| p.get("journal_issue"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let p_start = pub_info
        .and_then(|p| p.get("page_start"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let p_end = pub_info
        .and_then(|p| p.get("page_end"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let artid = pub_info
        .and_then(|p| p.get("artid"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let pages = match (p_start, p_end, artid) {
        (Some(s), Some(e), _) if s != e => Some(normalize_pages(&format!("{s}--{e}"))),
        (Some(s), _, _) => Some(s.to_string()),
        (None, None, Some(a)) => Some(a.to_string()),
        _ => None,
    };

    let year = pub_info
        .and_then(|p| p.get("year"))
        .and_then(Value::as_i64)
        .or_else(|| {
            meta.get("preprint_date")
                .and_then(Value::as_str)
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse::<i64>().ok())
        });

    let imprint = meta
        .get("imprints")
        .and_then(Value::as_array)
        .and_then(|a| a.first());
    let publisher = imprint
        .and_then(|i| i.get("publisher"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let address = imprint
        .and_then(|i| i.get("place"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let isbn = meta
        .get("isbns")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|x| x.get("value"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let abstract_text = meta
        .get("abstracts")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|ab| ab.get("value"))
        .and_then(Value::as_str)
        .map(clean_metadata_text)
        .filter(|s| !s.is_empty());

    let rec_id = meta
        .get("control_number")
        .and_then(|c| c.as_i64().map(|n| n.to_string()))
        .or_else(|| hit_id.and_then(Value::as_str).map(ToOwned::to_owned));

    let url = doi
        .as_ref()
        .map(|d| format!("https://doi.org/{d}"))
        .or_else(|| rec_id.map(|id| format!("https://inspirehep.net/literature/{id}")));

    Some(BibliographicRecord {
        work_type,
        title,
        authors,
        journal_abbrev,
        journal_full,
        booktitle,
        volume,
        issue_number,
        year,
        pages,
        publisher,
        address,
        isbn,
        doi,
        arxiv_id,
        url,
        abstract_text,
        source: "inspirehep".to_string(),
        license_note: Some(INSPIRE_LICENSE_NOTE.to_string()),
        ..BibliographicRecord::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inspirehep_literature_hit() {
        let sample = r#"{
          "hits": {
            "hits": [
              {
                "id": "2104567",
                "metadata": {
                  "control_number": 2104567,
                  "document_type": ["article"],
                  "titles": [{ "title": "Symplectic Integrators for Hamiltonian Lattice Gauge Theory" }],
                  "authors": [
                    { "full_name": "Hairer, Ernst", "last_name": "Hairer", "first_name": "Ernst" },
                    { "full_name": "Lubich, Christian" }
                  ],
                  "dois": [{ "value": "10.1007/JHEP05(2023)112" }],
                  "arxiv_eprints": [{ "value": "2302.04567" }],
                  "publication_info": [
                    {
                      "journal_title": "JHEP",
                      "journal_volume": "05",
                      "page_start": "112",
                      "year": 2023
                    }
                  ],
                  "abstracts": [{ "value": "Long-time energy conservation via splitting methods." }]
                }
              }
            ]
          }
        }"#;

        let ids = ExtractedIdentifiers {
            arxiv_id: Some("2302.04567".to_string()),
            ..ExtractedIdentifiers::default()
        };
        let hits = parse_inspirehep_response(sample, &ids).expect("parse inspire");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].score >= 0.98);
        assert_eq!(hits[0].record.authors.len(), 2);
        assert_eq!(hits[0].record.authors[1].family_name, "Lubich");
        assert_eq!(hits[0].record.doi.as_deref(), Some("10.1007/jhep05(2023)112"));
        assert_eq!(hits[0].record.year, Some(2023));
    }
}
