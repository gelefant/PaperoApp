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

const EUROPEPMC_SEARCH_URL: &str = "https://www.ebi.ac.uk/europepmc/webservices/rest/search";
const EUROPEPMC_LICENSE_NOTE: &str = "Open Metadata (Europe PMC)";

#[derive(Debug)]
pub struct EuropePmcClient {
    http: Client,
    limiter: SequentialRateLimiter,
}

impl EuropePmcClient {
    pub fn new(user_agent: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| CoreError::Network(format!("europepmc client init: {e}")))?;
        Ok(Self {
            http,
            limiter: SequentialRateLimiter::new(Duration::from_millis(500)),
        })
    }

    pub async fn lookup(
        &self,
        db: &Database,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        if let Some(doi) = ids
            .doi
            .as_deref()
            .filter(|d| !d.starts_with("10.48550/"))
        {
            if let Some(body) = self
                .fetch_europepmc_raw(db, &format!("DOI:\"{doi}\""), &format!("doi:{doi}"))
                .await?
            {
                let hits = parse_europepmc_response(&body, ids)?;
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
            format!("TITLE:\"{clean_ti}\" AND AUTH:\"{surname}\"")
        } else {
            format!("TITLE:\"{clean_ti}\"")
        };
        let cache_key = format!("q:{}", normalize_math_text(&query_str));
        let Some(body) = self.fetch_europepmc_raw(db, &query_str, &cache_key).await? else {
            return Ok(Vec::new());
        };

        parse_europepmc_response(&body, ids)
    }

    async fn fetch_europepmc_raw(
        &self,
        db: &Database,
        query_str: &str,
        cache_key: &str,
    ) -> Result<Option<String>> {
        if let Some(c) = load_http_cache(db, "europepmc", cache_key)? {
            if c.status_code >= 400 {
                return Ok(None);
            }
            return Ok(Some(c.body));
        }

        self.limiter.acquire().await;
        let req_url = url::Url::parse_with_params(
            EUROPEPMC_SEARCH_URL,
            &[
                ("query", query_str),
                ("format", "json"),
                ("pageSize", "5"),
                ("resultType", "core"),
            ],
        )
        .map_err(|e| CoreError::Network(format!("europepmc url build: {e}")))?;
        let resp = self
            .http
            .get(req_url)
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("europepmc request: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| CoreError::Network(format!("europepmc read: {e}")))?;
        if status < 400 || status == 404 {
            store_http_cache(db, "europepmc", cache_key, status, &text)?;
        }
        if status >= 400 {
            return Ok(None);
        }
        Ok(Some(text))
    }
}

pub fn parse_europepmc_response(
    body: &str,
    ids: &ExtractedIdentifiers,
) -> Result<Vec<CandidateMatch>> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| CoreError::Network(format!("europepmc json: {e}")))?;
    let mut out = Vec::new();
    let results = v
        .get("resultList")
        .and_then(|rl| rl.get("result"))
        .and_then(Value::as_array);
    for item in results.into_iter().flatten() {
        if let Some(rec) = parse_europepmc_item(item) {
            let score = score_candidate(&rec, ids);
            out.push(CandidateMatch {
                score,
                source: "europepmc".to_string(),
                record: rec,
            });
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

fn parse_europepmc_item(item: &Value) -> Option<BibliographicRecord> {
    let raw_title = item.get("title").and_then(Value::as_str)?;
    let title = clean_metadata_text(raw_title)
        .trim_end_matches('.')
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }

    let mut authors = Vec::new();
    for a in item
        .get("authorList")
        .and_then(|al| al.get("author"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(60)
    {
        let last = a
            .get("lastName")
            .and_then(Value::as_str)
            .map(clean_metadata_text)
            .unwrap_or_default();
        let first = a
            .get("firstName")
            .and_then(Value::as_str)
            .map(clean_metadata_text)
            .unwrap_or_default();
        if !last.is_empty() || !first.is_empty() {
            authors.push(BibliographicAuthor {
                family_name: last,
                given_name: first,
                role: "author".to_string(),
            });
        } else if let Some(full) = a.get("fullName").and_then(Value::as_str) {
            if let Some(parsed) = split_full_author_name(full, "author") {
                authors.push(parsed);
            }
        }
    }

    let pub_type_raw = item
        .get("pubTypeList")
        .and_then(|pt| pt.get("pubType"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .or_else(|| item.get("pubType").and_then(Value::as_str))
        .unwrap_or("")
        .to_ascii_lowercase();

    let work_type = if pub_type_raw.contains("preprint") {
        WorkType::Misc
    } else if pub_type_raw.contains("book") {
        WorkType::Book
    } else {
        WorkType::Article
    };

    let j_info = item.get("journalInfo");
    let j_obj = j_info.and_then(|j| j.get("journal"));
    let journal_full = j_obj
        .and_then(|j| j.get("title"))
        .and_then(Value::as_str)
        .map(clean_metadata_text)
        .filter(|s| !s.is_empty());
    let journal_abbrev = j_obj
        .and_then(|j| {
            j.get("isoAbbreviation")
                .or_else(|| j.get("medlineAbbreviation"))
        })
        .and_then(Value::as_str)
        .map(clean_metadata_text)
        .filter(|s| !s.is_empty());
    let issn = j_obj
        .and_then(|j| j.get("issn"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let volume = j_info
        .and_then(|j| j.get("volume"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let issue_number = j_info
        .and_then(|j| j.get("issue"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let year = j_info
        .and_then(|j| j.get("yearOfPublication"))
        .and_then(Value::as_i64)
        .or_else(|| {
            item.get("pubYear")
                .and_then(Value::as_str)
                .and_then(|y| y.trim().parse::<i64>().ok())
        });

    let pages = item
        .get("pageInfo")
        .and_then(Value::as_str)
        .map(normalize_pages);

    let doi = item
        .get("doi")
        .and_then(Value::as_str)
        .and_then(normalize_doi);

    let abstract_text = item
        .get("abstractText")
        .and_then(Value::as_str)
        .map(clean_metadata_text)
        .filter(|s| !s.is_empty());

    let url = doi
        .as_ref()
        .map(|d| format!("https://doi.org/{d}"))
        .or_else(|| {
            let src = item.get("source").and_then(Value::as_str)?;
            let id = item.get("id").and_then(Value::as_str)?;
            Some(format!("https://europepmc.org/article/{src}/{id}"))
        });

    Some(BibliographicRecord {
        work_type,
        title,
        authors,
        journal_abbrev,
        journal_full,
        volume,
        issue_number,
        year,
        pages,
        issn,
        doi,
        url,
        abstract_text,
        source: "europepmc".to_string(),
        license_note: Some(EUROPEPMC_LICENSE_NOTE.to_string()),
        ..BibliographicRecord::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_europepmc_core_result() {
        let sample = r#"{
          "resultList": {
            "result": [
              {
                "id": "34567890",
                "source": "MED",
                "title": "Mathematical modeling of cardiac electrophysiology using finite elements.",
                "pubYear": "2021",
                "pageInfo": "104-122",
                "doi": "10.1371/journal.pcbi.1009123",
                "abstractText": "<h4>Background</h4> Couplings between PDEs and ODEs.",
                "authorList": {
                  "author": [
                    { "firstName": "Alfio", "lastName": "Quarteroni" }
                  ]
                },
                "journalInfo": {
                  "volume": "17",
                  "issue": "8",
                  "yearOfPublication": 2021,
                  "journal": {
                    "title": "PLoS Computational Biology",
                    "isoAbbreviation": "PLoS Comput Biol",
                    "issn": "1553-734X"
                  }
                }
              }
            ]
          }
        }"#;

        let ids = ExtractedIdentifiers {
            doi: Some("10.1371/journal.pcbi.1009123".to_string()),
            ..ExtractedIdentifiers::default()
        };
        let hits = parse_europepmc_response(sample, &ids).expect("parse europepmc");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].score >= 0.98);
        assert_eq!(hits[0].record.authors[0].family_name, "Quarteroni");
        assert_eq!(hits[0].record.pages.as_deref(), Some("104--122"));
        assert_eq!(
            hits[0].record.abstract_text.as_deref(),
            Some("Background Couplings between PDEs and ODEs.")
        );
    }
}
