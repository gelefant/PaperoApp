use crate::credentials::load_secret_key;
use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::metadata::extract::normalize_isbn;
use crate::metadata::limiter::{load_http_cache, store_http_cache, SequentialRateLimiter};
use crate::metadata::models::{
    BibliographicAuthor, BibliographicRecord, CandidateMatch, ExtractedIdentifiers, WorkType,
};
use crate::metadata::mrlookup::score_candidate;
use crate::text_norm::normalize_math_text;
use reqwest::Client;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const GOOGLE_BOOKS_BASE: &str = "https://www.googleapis.com/books/v1/volumes";
pub const KEY_GOOGLEBOOKS_API: &str = "googlebooks_api_key";

/// Google Books Volumes v1 client for books & monographs.
/// Works without an API key (using an instant 10-minute circuit-breaker if Google's shared
/// anonymous IP quota returns HTTP 429/403 so bulk syncs never stall) and also automatically
/// uses `googlebooks_api_key` from OS Keychain if present.
#[derive(Debug)]
pub struct GoogleBooksClient {
    http: Client,
    limiter: SequentialRateLimiter,
    suspended_until: Mutex<Option<Instant>>,
}

impl GoogleBooksClient {
    pub fn new(user_agent: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(12))
            .build()
            .map_err(|e| CoreError::Network(format!("googlebooks client init: {e}")))?;
        Ok(Self {
            http,
            limiter: SequentialRateLimiter::new(Duration::from_millis(250)),
            suspended_until: Mutex::new(None),
        })
    }

    async fn is_suspended(&self) -> bool {
        let mut guard = self.suspended_until.lock().await;
        if let Some(until) = *guard {
            if Instant::now() < until {
                return true;
            }
            *guard = None;
        }
        false
    }

    async fn trip_circuit_breaker(&self) {
        let mut guard = self.suspended_until.lock().await;
        *guard = Some(Instant::now() + Duration::from_secs(600));
    }

    pub async fn lookup_book(
        &self,
        db: &Database,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        if !ids.looks_like_book && ids.isbn.is_none() {
            return Ok(Vec::new());
        }

        if let Some(isbn) = ids.isbn.as_deref() {
            let cache_key = format!("isbn:{isbn}");
            if let Some(cached) = load_http_cache(db, "googlebooks", &cache_key)? {
                if cached.status_code < 400 {
                    let hits = parse_googlebooks_response(&cached.body, Some(isbn));
                    if !hits.is_empty() {
                        return Ok(hits
                            .into_iter()
                            .map(|rec| CandidateMatch {
                                score: 0.98,
                                source: "googlebooks".to_string(),
                                record: rec,
                            })
                            .collect());
                    }
                }
            } else if !self.is_suspended().await {
                let mut url = format!(
                    "{GOOGLE_BOOKS_BASE}?q=isbn:{}&maxResults=3",
                    url::form_urlencoded::byte_serialize(isbn.as_bytes()).collect::<String>()
                );
                if let Ok(Some(secret)) = load_secret_key(KEY_GOOGLEBOOKS_API) {
                    url.push_str("&key=");
                    url.push_str(
                        &url::form_urlencoded::byte_serialize(secret.expose().trim().as_bytes())
                            .collect::<String>(),
                    );
                }
                if let Ok((status, body)) = self.fetch_url(&url).await {
                    if status == 429 || status == 403 {
                        self.trip_circuit_breaker().await;
                        return Ok(Vec::new());
                    }
                    if status < 500 {
                        store_http_cache(db, "googlebooks", &cache_key, status, &body)?;
                    }
                    if status < 400 {
                        let hits = parse_googlebooks_response(&body, Some(isbn));
                        if !hits.is_empty() {
                            return Ok(hits
                                .into_iter()
                                .map(|rec| CandidateMatch {
                                    score: 0.98,
                                    source: "googlebooks".to_string(),
                                    record: rec,
                                })
                                .collect());
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
        let cache_key = format!(
            "book:ti={}:au={}",
            normalize_math_text(ti),
            normalize_math_text(au)
        );

        let body = if let Some(cached) = load_http_cache(db, "googlebooks", &cache_key)? {
            if cached.status_code >= 400 {
                return Ok(Vec::new());
            }
            cached.body
        } else {
            if self.is_suspended().await {
                return Ok(Vec::new());
            }
            let q_expr = if au.is_empty() {
                format!("intitle:\"{ti}\"")
            } else {
                format!("intitle:\"{ti}\" inauthor:\"{au}\"")
            };
            let mut url = format!(
                "{GOOGLE_BOOKS_BASE}?q={}&printType=books&maxResults=5",
                url::form_urlencoded::byte_serialize(q_expr.as_bytes()).collect::<String>()
            );
            if let Ok(Some(secret)) = load_secret_key(KEY_GOOGLEBOOKS_API) {
                url.push_str("&key=");
                url.push_str(
                    &url::form_urlencoded::byte_serialize(secret.expose().trim().as_bytes())
                        .collect::<String>(),
                );
            }
            let (status, text) = self.fetch_url(&url).await?;
            if status == 429 || status == 403 {
                self.trip_circuit_breaker().await;
                return Ok(Vec::new());
            }
            if status < 500 {
                store_http_cache(db, "googlebooks", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        let mut out: Vec<CandidateMatch> = parse_googlebooks_response(&body, None)
            .into_iter()
            .map(|rec| CandidateMatch {
                score: score_candidate(&rec, ids),
                source: "googlebooks".to_string(),
                record: rec,
            })
            .collect();
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(out)
    }

    async fn fetch_url(&self, url: &str) -> Result<(u16, String)> {
        self.limiter.acquire().await;
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("googlebooks request: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| CoreError::Network(format!("googlebooks read body: {e}")))?;
        Ok((status, text))
    }
}

fn split_google_author(raw: &str) -> Option<BibliographicAuthor> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((fam, giv)) = trimmed.split_once(',') {
        return Some(BibliographicAuthor {
            family_name: fam.trim().to_string(),
            given_name: giv.trim().to_string(),
            role: "author".to_string(),
        });
    }
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() >= 2 {
        Some(BibliographicAuthor {
            family_name: parts.last().unwrap().to_string(),
            given_name: parts[..parts.len() - 1].join(" "),
            role: "author".to_string(),
        })
    } else {
        Some(BibliographicAuthor {
            family_name: trimmed.to_string(),
            given_name: String::new(),
            role: "author".to_string(),
        })
    }
}

pub fn parse_googlebooks_response(
    body: &str,
    fallback_isbn: Option<&str>,
) -> Vec<BibliographicRecord> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let Some(items) = root.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(vi) = item.get("volumeInfo") else {
            continue;
        };
        let Some(main_title) = vi
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let subtitle = vi
            .get("subtitle")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let title = match subtitle {
            Some(sub) if !normalize_math_text(main_title).contains(&normalize_math_text(sub)) => {
                format!("{main_title}: {sub}")
            }
            _ => main_title.to_string(),
        };

        let authors: Vec<BibliographicAuthor> = vi
            .get("authors")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .filter_map(split_google_author)
                    .collect()
            })
            .unwrap_or_default();

        let publisher = vi
            .get("publisher")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);

        let year = vi
            .get("publishedDate")
            .and_then(Value::as_str)
            .and_then(|d| d.get(0..4))
            .and_then(|y| y.parse::<i64>().ok())
            .filter(|&y| (1500..=2100).contains(&y));

        let pages = vi
            .get("pageCount")
            .and_then(Value::as_u64)
            .filter(|&n| n > 0)
            .map(|n| n.to_string());

        let mut isbn: Option<String> = None;
        if let Some(idents) = vi.get("industryIdentifiers").and_then(Value::as_array) {
            for id_obj in idents {
                let id_type = id_obj.get("type").and_then(Value::as_str).unwrap_or("");
                let raw_val = id_obj.get("identifier").and_then(Value::as_str).unwrap_or("");
                if (id_type == "ISBN_13" || id_type == "ISBN_10")
                    && !raw_val.is_empty()
                {
                    if let Some(norm) = normalize_isbn(raw_val) {
                        let prefer = isbn.as_ref().is_none_or(|prev| prev.len() < norm.len());
                        if prefer {
                            isbn = Some(norm);
                        }
                    }
                }
            }
        }
        if isbn.is_none() {
            isbn = fallback_isbn.and_then(normalize_isbn);
        }

        let abstract_text = vi
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);

        let url = vi
            .get("canonicalVolumeLink")
            .or_else(|| vi.get("infoLink"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);

        out.push(BibliographicRecord {
            work_type: WorkType::Book,
            title,
            authors,
            publisher,
            year,
            pages,
            isbn,
            abstract_text,
            url,
            source: "googlebooks".to_string(),
            ..Default::default()
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_google_books_volume_info_with_publisher_isbn13_and_pages() {
        let json = r#"{
          "totalItems": 1,
          "items": [
            {
              "volumeInfo": {
                "title": "Principles of Mathematical Analysis",
                "authors": ["Walter Rudin"],
                "publisher": "McGraw-Hill",
                "publishedDate": "1976-01-01",
                "pageCount": 342,
                "industryIdentifiers": [
                  { "type": "ISBN_10", "identifier": "007054235X" },
                  { "type": "ISBN_13", "identifier": "9780070542358" }
                ],
                "canonicalVolumeLink": "https://books.google.com/books/about/Principles_of_Mathematical_Analysis.html?id=123"
              }
            }
          ]
        }"#;
        let recs = parse_googlebooks_response(json, None);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.title, "Principles of Mathematical Analysis");
        assert_eq!(r.authors[0].family_name, "Rudin");
        assert_eq!(r.authors[0].given_name, "Walter");
        assert_eq!(r.publisher.as_deref(), Some("McGraw-Hill"));
        assert_eq!(r.year, Some(1976));
        assert_eq!(r.pages.as_deref(), Some("342"));
        assert_eq!(r.isbn.as_deref(), Some("9780070542358"));
    }
}
