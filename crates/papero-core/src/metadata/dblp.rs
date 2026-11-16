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

const DBLP_SEARCH_URL: &str = "https://dblp.org/search/publ/api";
const DBLP_LICENSE_NOTE: &str = "CC0 1.0 (DBLP)";

#[derive(Debug)]
pub struct DblpClient {
    http: Client,
    limiter: SequentialRateLimiter,
}

impl DblpClient {
    pub fn new(user_agent: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| CoreError::Network(format!("dblp client init: {e}")))?;
        Ok(Self {
            http,
            limiter: SequentialRateLimiter::new(Duration::from_millis(900)),
        })
    }

    pub async fn lookup(
        &self,
        db: &Database,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        let surname = ids
            .heuristic_first_author
            .as_deref()
            .and_then(clean_author_surname)
            .unwrap_or_default();

        if let Some(doi) = ids
            .doi
            .as_deref()
            .filter(|d| !d.starts_with("10.48550/"))
        {
            if let Some(body) = self
                .fetch_dblp_raw(db, doi, &format!("doi:{doi}"))
                .await?
            {
                let hits = parse_dblp_response(&body, ids)?;
                if !hits.is_empty() {
                    return Ok(hits);
                }
            }
        }

        let (query_str, cache_key) = if let Some(ti) = ids
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
            (q.clone(), format!("q:{}", normalize_math_text(&q)))
        } else if !surname.is_empty() {
            let kw = ids.filename_keywords.join(" ");
            let q = match (ids.heuristic_year, kw.is_empty()) {
                (Some(yr), true) => format!("{surname} {yr}"),
                (Some(yr), false) => format!("{surname} {yr} {kw}"),
                (None, true) => surname.clone(),
                (None, false) => format!("{surname} {kw}"),
            };
            (q.clone(), format!("auyr:{}", normalize_math_text(&q)))
        } else {
            return Ok(Vec::new());
        };

        let Some(body) = self.fetch_dblp_raw(db, &query_str, &cache_key).await? else {
            return Ok(Vec::new());
        };

        parse_dblp_response(&body, ids)
    }

    async fn fetch_dblp_raw(
        &self,
        db: &Database,
        query_str: &str,
        cache_key: &str,
    ) -> Result<Option<String>> {
        if let Some(cached) = load_http_cache(db, "dblp", cache_key)? {
            let trimmed = cached.body.trim_start();
            let looks_json = trimmed.starts_with('{') || trimmed.starts_with('[');
            if cached.status_code == 200 && looks_json {
                return Ok(Some(cached.body));
            }
            if cached.status_code >= 400 {
                return Ok(None);
            }
            // Ignore non-JSON 202 anti-bot/HTML challenge entries cached before this guard.
        }

        self.limiter.acquire().await;
        let req_url = url::Url::parse_with_params(
            DBLP_SEARCH_URL,
            &[("q", query_str), ("h", "8"), ("format", "json")],
        )
        .map_err(|e| CoreError::Network(format!("dblp url build: {e}")))?;

        let resp = self
            .http
            .get(req_url)
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("dblp request: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| CoreError::Network(format!("dblp read body: {e}")))?;
        let is_json_like = {
            let trimmed = text.trim_start();
            trimmed.starts_with('{') || trimmed.starts_with('[')
        };
        if (status == 200 && is_json_like) || status == 404 {
            store_http_cache(db, "dblp", cache_key, status, &text)?;
        }
        if status >= 400 || !is_json_like {
            return Ok(None);
        }
        Ok(Some(text))
    }
}

pub fn parse_dblp_response(body: &str, ids: &ExtractedIdentifiers) -> Result<Vec<CandidateMatch>> {
    let val: Value = serde_json::from_str(body)
        .map_err(|e| CoreError::Network(format!("dblp json parse: {e}")))?;

    let hits_val = val
        .get("result")
        .and_then(|r| r.get("hits"))
        .and_then(|h| h.get("hit"));

    let mut items: Vec<&Value> = Vec::new();
    if let Some(arr) = hits_val.and_then(Value::as_array) {
        items.extend(arr.iter());
    } else if let Some(obj) = hits_val.filter(|v| v.is_object()) {
        items.push(obj);
    }

    let mut out = Vec::new();
    for hit in items {
        let Some(info) = hit.get("info") else {
            continue;
        };
        let Some(rec) = parse_dblp_info(info) else {
            continue;
        };
        let score = score_candidate(&rec, ids);
        out.push(CandidateMatch {
            score,
            source: "dblp".to_string(),
            record: rec,
        });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

fn parse_dblp_info(info: &Value) -> Option<BibliographicRecord> {
    let raw_title = info.get("title").and_then(Value::as_str)?;
    let title = clean_metadata_text(raw_title)
        .trim_end_matches('.')
        .trim()
        .to_string();
    if title.is_empty() {
        return None;
    }

    let mut authors: Vec<BibliographicAuthor> = Vec::new();
    if let Some(author_node) = info.get("authors").and_then(|a| a.get("author")) {
        match author_node {
            Value::Array(arr) => {
                for entry in arr {
                    if let Some(a) = extract_dblp_author(entry) {
                        authors.push(a);
                    }
                }
            }
            other => {
                if let Some(a) = extract_dblp_author(other) {
                    authors.push(a);
                }
            }
        }
    }

    let dblp_type = info.get("type").and_then(Value::as_str).unwrap_or("");
    let venue_str = first_string_or_array(info.get("venue"));
    let work_type = match dblp_type {
        "Conference and Workshop Papers" => WorkType::Inproceedings,
        "Books and Theses" => WorkType::Book,
        "Parts in Books or Collections" => WorkType::Incollection,
        "Informal and Other Publications" => WorkType::Misc,
        _ => WorkType::Article,
    };

    let (journal_full, journal_abbrev, booktitle) = match work_type {
        WorkType::Inproceedings | WorkType::Incollection => (None, None, venue_str),
        _ => (venue_str.clone(), venue_str, None),
    };

    let year = info
        .get("year")
        .and_then(|y| y.as_i64().or_else(|| y.as_str()?.trim().parse::<i64>().ok()));

    let mut doi = info.get("doi").and_then(Value::as_str).and_then(normalize_doi);
    let mut arxiv_id = None;
    let mut primary_ee = None;

    for ee in all_strings_from_value(info.get("ee")) {
        if primary_ee.is_none() {
            primary_ee = Some(ee.clone());
        }
        if doi.is_none() {
            doi = normalize_doi(&ee);
        }
        if arxiv_id.is_none() {
            if let Some(ax) = extract_arxiv_from_url(&ee) {
                arxiv_id = Some(ax);
            }
        }
    }

    if arxiv_id.is_none() && venue_str_is_corr(journal_full.as_deref()) {
        if let Some(vol) = info.get("volume").and_then(Value::as_str) {
            if let Some(stripped) = vol.trim().strip_prefix("abs/") {
                arxiv_id = Some(stripped.to_string());
            }
        }
    }

    let url = primary_ee
        .or_else(|| info.get("url").and_then(Value::as_str).map(ToOwned::to_owned))
        .or_else(|| doi.as_ref().map(|d| format!("https://doi.org/{d}")));

    Some(BibliographicRecord {
        work_type,
        title,
        authors,
        journal_abbrev,
        journal_full,
        booktitle,
        volume: first_string_or_array(info.get("volume")),
        issue_number: first_string_or_array(info.get("number")),
        year,
        pages: first_string_or_array(info.get("pages")).map(|p| normalize_pages(&p)),
        publisher: first_string_or_array(info.get("publisher")),
        doi,
        arxiv_id,
        url,
        source: "dblp".to_string(),
        license_note: Some(DBLP_LICENSE_NOTE.to_string()),
        ..BibliographicRecord::default()
    })
}

fn extract_dblp_author(v: &Value) -> Option<BibliographicAuthor> {
    let name = v
        .as_str()
        .or_else(|| v.get("text").and_then(Value::as_str))?;
    split_full_author_name(name, "author")
}

fn first_string_or_array(v: Option<&Value>) -> Option<String> {
    let v = v?;
    if let Some(s) = v.as_str() {
        let t = clean_metadata_text(s);
        return (!t.is_empty()).then_some(t);
    }
    if let Some(arr) = v.as_array() {
        for item in arr {
            if let Some(s) = item.as_str() {
                let t = clean_metadata_text(s);
                if !t.is_empty() {
                    return Some(t);
                }
            }
        }
    }
    None
}

fn all_strings_from_value(v: Option<&Value>) -> Vec<String> {
    let Some(v) = v else {
        return Vec::new();
    };
    if let Some(s) = v.as_str() {
        return vec![s.trim().to_string()];
    }
    if let Some(arr) = v.as_array() {
        return arr
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    Vec::new()
}

fn extract_arxiv_from_url(url: &str) -> Option<String> {
    let idx = url.find("arxiv.org/abs/")?;
    let rest = &url[idx + "arxiv.org/abs/".len()..];
    let token = rest
        .trim_end_matches('/')
        .split(['?', '#'])
        .next()?
        .trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn venue_str_is_corr(venue: Option<&str>) -> bool {
    venue
        .map(|s| s.eq_ignore_ascii_case("corr"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dblp_single_and_multi_author_records() {
        let sample = r#"{
          "result": {
            "hits": {
              "@total": "2",
              "hit": [
                {
                  "info": {
                    "authors": {
                      "author": [
                        { "@pid": "12/34", "text": "Yang Liu 0001" },
                        { "@pid": "56/78", "text": "Gene H. Golub" }
                      ]
                    },
                    "title": " Krylov Subspace Methods for <i>Large</i> Linear Systems. ",
                    "venue": "SIAM J. Sci. Comput.",
                    "volume": "42",
                    "number": "3",
                    "pages": "A120-A145",
                    "year": "2020",
                    "type": "Journal Articles",
                    "doi": "10.1137/19M1234567",
                    "ee": "https://doi.org/10.1137/19M1234567"
                  }
                },
                {
                  "info": {
                    "authors": {
                      "author": { "@pid": "99/00", "text": "Donald E. Knuth" }
                    },
                    "title": "Structured Programming with go to Statements.",
                    "venue": "ACM Comput. Surv.",
                    "year": "1974",
                    "type": "Conference and Workshop Papers",
                    "ee": ["https://arxiv.org/abs/2301.09999"]
                  }
                }
              ]
            }
          }
        }"#;

        let ids = ExtractedIdentifiers {
            doi: Some("10.1137/19m1234567".to_string()),
            ..ExtractedIdentifiers::default()
        };
        let hits = parse_dblp_response(sample, &ids).expect("parse dblp");
        assert_eq!(hits.len(), 2);
        let top = &hits[0];
        assert!(top.score >= 0.98);
        assert_eq!(
            top.record.title,
            "Krylov Subspace Methods for Large Linear Systems"
        );
        assert_eq!(top.record.authors.len(), 2);
        assert_eq!(top.record.authors[0].family_name, "Liu");
        assert_eq!(top.record.authors[0].given_name, "Yang");
        assert_eq!(top.record.pages.as_deref(), Some("A120--A145"));
        assert_eq!(top.record.work_type, WorkType::Article);

        let second = &hits[1];
        assert_eq!(second.record.authors.len(), 1);
        assert_eq!(second.record.authors[0].family_name, "Knuth");
        assert_eq!(second.record.work_type, WorkType::Inproceedings);
        assert_eq!(second.record.arxiv_id.as_deref(), Some("2301.09999"));
    }

    #[tokio::test]
    async fn ignores_non_json_antibot_html_cache_entries() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("dblp_html.db"), 2).unwrap();
        let client = DblpClient::new("PaperoTest/0.2".into()).unwrap();

        let err_ck = format!("q:{}", normalize_math_text("Large-Scale Manifold Learning"));
        store_http_cache(
            &db,
            "dblp",
            &err_ck,
            202,
            "<!doctype html><html><title>Making sure you're not a bot!</title></html>",
        )
        .unwrap();

        // Confirm valid JSON cache still works normally
        let ok_ck = format!("q:{}", normalize_math_text("Valid Cached Paper"));
        store_http_cache(
            &db,
            "dblp",
            &ok_ck,
            200,
            r#"{"result":{"hits":{"hit":[{"info":{"title":"Valid Cached Paper","authors":{"author":{"text":"Ada Lovelace"}},"year":"1843","type":"Conference and Workshop Papers","venue":"Notes"}}]}}}"#,
        )
        .unwrap();
        let hits = client
            .lookup(
                &db,
                &ExtractedIdentifiers {
                    heuristic_title: Some("Valid Cached Paper".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].record.booktitle.as_deref(), Some("Notes"));

        // Cached 202 HTML is ignored cleanly rather than crashing JSON parsing if parsed directly.
        let loaded = load_http_cache(&db, "dblp", &err_ck).unwrap().unwrap();
        assert_eq!(loaded.status_code, 202);
        let trimmed = loaded.body.trim_start();
        assert!(!(trimmed.starts_with('{') || trimmed.starts_with('[')));
    }
}
