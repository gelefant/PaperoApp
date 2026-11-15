use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::metadata::limiter::{load_http_cache, store_http_cache, SequentialRateLimiter};
use crate::metadata::models::{
    clean_metadata_text, normalize_doi, normalize_pages, split_full_author_name,
    BibliographicAuthor, BibliographicRecord, CandidateMatch, ExtractedIdentifiers, WorkType,
};
use crate::metadata::mrlookup::score_candidate;
use crate::text_norm::normalize_math_text;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use url::form_urlencoded;

const DATACITE_API: &str = "https://api.datacite.org/dois";
const DATACITE_LICENSE_NOTE: &str = "CC0 1.0 (DataCite)";

#[derive(Debug)]
pub struct DataCiteClient {
    http: Client,
    limiter: SequentialRateLimiter,
}

impl DataCiteClient {
    pub fn new(user_agent: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| CoreError::Network(format!("datacite client init: {e}")))?;
        Ok(Self {
            http,
            limiter: SequentialRateLimiter::new(Duration::from_millis(350)),
        })
    }

    pub async fn lookup(
        &self,
        db: &Database,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        if let Some(doi) = ids.doi.as_deref() {
            let cache_key = format!("doi:{doi}");
            let maybe_body = if let Some(c) = load_http_cache(db, "datacite", &cache_key)? {
                (c.status_code < 400).then_some(c.body)
            } else {
                self.limiter.acquire().await;
                let encoded: String = form_urlencoded::byte_serialize(doi.as_bytes()).collect();
                let url = format!("{DATACITE_API}/{encoded}");
                let resp = self
                    .http
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| CoreError::Network(format!("datacite doi request: {e}")))?;
                let status = resp.status().as_u16();
                let text = resp
                    .text()
                    .await
                    .map_err(|e| CoreError::Network(format!("datacite doi read: {e}")))?;
                if status < 400 || status == 404 {
                    store_http_cache(db, "datacite", &cache_key, status, &text)?;
                }
                (status < 400).then_some(text)
            };

            if let Some(body) = maybe_body {
                if let Ok(v) = serde_json::from_str::<Value>(&body) {
                    if let Some(data) = v.get("data") {
                        if let Some(rec) = parse_datacite_item(data, false) {
                            return Ok(vec![CandidateMatch {
                                score: 0.98,
                                source: "datacite".to_string(),
                                record: rec,
                            }]);
                        }
                    }
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
        let au = ids.heuristic_first_author.as_deref().unwrap_or("").trim();
        let query_str = if au.is_empty() {
            ti.to_string()
        } else {
            format!("{ti} {au}")
        };
        let cache_key = format!("q:{}", normalize_math_text(&query_str));

        let body = if let Some(c) = load_http_cache(db, "datacite", &cache_key)? {
            if c.status_code >= 400 {
                return Ok(Vec::new());
            }
            c.body
        } else {
            self.limiter.acquire().await;
            let req_url = url::Url::parse_with_params(
                DATACITE_API,
                &[("query", query_str.as_str()), ("page[size]", "5")],
            )
            .map_err(|e| CoreError::Network(format!("datacite list url: {e}")))?;
            let resp = self
                .http
                .get(req_url)
                .send()
                .await
                .map_err(|e| CoreError::Network(format!("datacite list request: {e}")))?;
            let status = resp.status().as_u16();
            let text = resp
                .text()
                .await
                .map_err(|e| CoreError::Network(format!("datacite list read: {e}")))?;
            if status < 400 || status == 404 {
                store_http_cache(db, "datacite", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        parse_datacite_list_response(&body, ids)
    }
}

pub fn parse_datacite_list_response(
    body: &str,
    ids: &ExtractedIdentifiers,
) -> Result<Vec<CandidateMatch>> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| CoreError::Network(format!("datacite list json: {e}")))?;
    let mut out = Vec::new();
    if let Some(arr) = v.get("data").and_then(Value::as_array) {
        for item in arr {
            if let Some(rec) = parse_datacite_item(item, true) {
                let score = score_candidate(&rec, ids);
                out.push(CandidateMatch {
                    score,
                    source: "datacite".to_string(),
                    record: rec,
                });
            }
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

fn parse_datacite_item(item: &Value, skip_non_text_types: bool) -> Option<BibliographicRecord> {
    let attr = item.get("attributes").unwrap_or(item);

    let res_type = attr
        .get("types")
        .and_then(|t| t.get("resourceTypeGeneral"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if skip_non_text_types
        && matches!(
            res_type,
            "Dataset"
                | "Software"
                | "Image"
                | "Audiovisual"
                | "Model"
                | "Workflow"
                | "Collection"
                | "PhysicalObject"
                | "InteractiveResource"
        )
    {
        return None;
    }

    let title = attr
        .get("titles")
        .and_then(Value::as_array)
        .and_then(|arr| {
            arr.iter()
                .filter_map(|t| t.get("title").and_then(Value::as_str))
                .map(clean_metadata_text)
                .find(|s| !s.is_empty())
        })?;

    let bib_type = attr
        .get("types")
        .and_then(|t| t.get("bibtex"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let work_type = match (res_type, bib_type) {
        ("Book", _) | (_, "book") => WorkType::Book,
        ("BookChapter", _) | (_, "incollection") => WorkType::Incollection,
        ("ConferencePaper", _) | (_, "inproceedings") => WorkType::Inproceedings,
        ("Dissertation", _) | (_, "phdthesis") => WorkType::Phdthesis,
        ("Preprint", _) | (_, "misc") => WorkType::Misc,
        _ => WorkType::Article,
    };

    let mut authors = Vec::new();
    for c in attr
        .get("creators")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(60)
    {
        let fam = c
            .get("familyName")
            .and_then(Value::as_str)
            .map(clean_metadata_text)
            .unwrap_or_default();
        let giv = c
            .get("givenName")
            .and_then(Value::as_str)
            .map(clean_metadata_text)
            .unwrap_or_default();
        if !fam.is_empty() || !giv.is_empty() {
            authors.push(BibliographicAuthor {
                family_name: fam,
                given_name: giv,
                role: "author".to_string(),
            });
        } else if let Some(name) = c.get("name").and_then(Value::as_str) {
            if let Some(parsed) = split_full_author_name(name, "author") {
                authors.push(parsed);
            }
        }
    }

    let year = attr
        .get("publicationYear")
        .and_then(|y| y.as_i64().or_else(|| y.as_str()?.parse::<i64>().ok()));

    let publisher = attr.get("publisher").and_then(|p| {
        p.as_str()
            .or_else(|| p.get("name").and_then(Value::as_str))
            .map(clean_metadata_text)
            .filter(|s| !s.is_empty())
    });

    let container = attr.get("container");
    let container_title = container
        .and_then(|c| c.get("title"))
        .and_then(Value::as_str)
        .map(clean_metadata_text)
        .filter(|s| !s.is_empty());

    let (journal_full, booktitle) = match work_type {
        WorkType::Inproceedings | WorkType::Incollection => (None, container_title),
        _ => (container_title, None),
    };

    let volume = container
        .and_then(|c| c.get("volume"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let issue_number = container
        .and_then(|c| c.get("issue"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let fp = container
        .and_then(|c| c.get("firstPage"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let lp = container
        .and_then(|c| c.get("lastPage"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let pages = match (fp, lp) {
        (Some(a), Some(b)) if a != b => Some(normalize_pages(&format!("{a}--{b}"))),
        (Some(a), _) => Some(a.to_string()),
        _ => None,
    };

    let doi = attr
        .get("doi")
        .and_then(Value::as_str)
        .or_else(|| item.get("id").and_then(Value::as_str))
        .and_then(normalize_doi);

    let mut arxiv_id = doi
        .as_deref()
        .and_then(|d| d.strip_prefix("10.48550/arxiv."))
        .map(ToOwned::to_owned);

    if arxiv_id.is_none() {
        if let Some(idents) = attr.get("identifiers").and_then(Value::as_array) {
            for id_obj in idents {
                let id_type = id_obj
                    .get("identifierType")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if id_type.eq_ignore_ascii_case("arxiv") {
                    if let Some(v) = id_obj.get("identifier").and_then(Value::as_str) {
                        let cleaned = v.trim().strip_prefix("arXiv:").unwrap_or(v.trim());
                        if !cleaned.is_empty() {
                            arxiv_id = Some(cleaned.to_string());
                            break;
                        }
                    }
                }
            }
        }
    }

    let abstract_text = attr
        .get("descriptions")
        .and_then(Value::as_array)
        .and_then(|arr| {
            arr.iter()
                .find(|d| {
                    d.get("descriptionType")
                        .and_then(Value::as_str)
                        .map(|t| t.eq_ignore_ascii_case("abstract"))
                        .unwrap_or(true)
                })
                .and_then(|d| d.get("description"))
                .and_then(Value::as_str)
                .map(clean_metadata_text)
                .filter(|s| !s.is_empty())
        });

    let url = attr
        .get("url")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| doi.as_ref().map(|d| format!("https://doi.org/{d}")));

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
        doi,
        arxiv_id,
        url,
        abstract_text,
        source: "datacite".to_string(),
        license_note: Some(DATACITE_LICENSE_NOTE.to_string()),
        ..BibliographicRecord::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_datacite_arxiv_and_eth_doi_and_skips_datasets_on_search() {
        let sample = r#"{
          "data": [
            {
              "id": "10.3929/ethz-b-000412345",
              "attributes": {
                "doi": "10.3929/ethz-b-000412345",
                "titles": [{ "title": "Structure-Preserving Finite Element Exterior Calculus" }],
                "creators": [
                  { "familyName": "Hiptmair", "givenName": "Ralf" }
                ],
                "publisher": { "name": "ETH Zurich" },
                "publicationYear": 2022,
                "types": { "resourceTypeGeneral": "Preprint", "bibtex": "misc" },
                "descriptions": [
                  { "descriptionType": "Abstract", "description": "Discrete differential forms on simplicial meshes." }
                ]
              }
            },
            {
              "id": "10.5281/zenodo.999999",
              "attributes": {
                "doi": "10.5281/zenodo.999999",
                "titles": [{ "title": "Replication Dataset" }],
                "types": { "resourceTypeGeneral": "Dataset" }
              }
            }
          ]
        }"#;

        let ids = ExtractedIdentifiers {
            heuristic_title: Some(
                "Structure-Preserving Finite Element Exterior Calculus".to_string(),
            ),
            heuristic_first_author: Some("Hiptmair".to_string()),
            ..ExtractedIdentifiers::default()
        };
        let hits = parse_datacite_list_response(sample, &ids).expect("parse datacite list");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.authors[0].family_name, "Hiptmair");
        assert_eq!(hits[0].record.publisher.as_deref(), Some("ETH Zurich"));
        assert_eq!(hits[0].record.year, Some(2022));
    }
}
