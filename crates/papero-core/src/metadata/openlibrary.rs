use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::metadata::limiter::{load_http_cache, store_http_cache, OpenLibraryLimiter};
use crate::metadata::models::{
    BibliographicAuthor, BibliographicRecord, CandidateMatch, ExtractedIdentifiers, WorkType,
};
use crate::metadata::mrlookup::score_candidate;
use crate::text_norm::normalize_math_text;
use reqwest::Client;
use serde_json::Value;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

const OPENLIBRARY_BASE: &str = "https://openlibrary.org";

pub type MockHttpTransport = Arc<dyn Fn(&str) -> (u16, String) + Send + Sync>;

/// Books-ONLY fallback client enforcing Open Library Terms of Use:
/// - No bulk dump downloads; strictly one query per unresolved book in user's library.
/// - Sequential concurrency = 1 with `<= 1 req / s` enforced by `OpenLibraryLimiter`.
/// - Permanently cached per ISBN / title+author query in SQLite `metadata_http_cache`.
/// - Exponential backoff + circuit breaker suspension after consecutive 429/5xx responses.
pub struct OpenLibraryClient {
    http: Client,
    base_url: String,
    pub limiter: Arc<OpenLibraryLimiter>,
    mock_transport: Option<MockHttpTransport>,
}

impl fmt::Debug for OpenLibraryClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenLibraryClient")
            .field("base_url", &self.base_url)
            .field("limiter", &self.limiter)
            .field("has_mock_transport", &self.mock_transport.is_some())
            .finish()
    }
}

impl OpenLibraryClient {
    pub fn new(app_version: &str, polite_email: Option<String>) -> Result<Self> {
        let ua = match polite_email.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(mail) => format!("Papero/{app_version} ({mail})"),
            None => format!("Papero/{app_version}"),
        };
        let http = Client::builder()
            .user_agent(ua)
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| CoreError::Network(format!("openlibrary client init: {e}")))?;
        Ok(Self {
            http,
            base_url: OPENLIBRARY_BASE.to_string(),
            limiter: Arc::new(OpenLibraryLimiter::default_tos()),
            mock_transport: None,
        })
    }

    pub fn with_base_and_limiter(
        mut self,
        base_url: impl Into<String>,
        limiter: Arc<OpenLibraryLimiter>,
    ) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self.limiter = limiter;
        self
    }

    pub fn with_mock_transport(mut self, transport: MockHttpTransport) -> Self {
        self.mock_transport = Some(transport);
        self
    }

    /// Queries Open Library ONLY when the item appears to be a book (`looks_like_book || isbn.is_some()`).
    pub async fn lookup_book(
        &self,
        db: &Database,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        if !ids.looks_like_book && ids.isbn.is_none() {
            return Ok(Vec::new());
        }

        if let Some(isbn) = ids.isbn.as_deref() {
            let cache_key = format!("isbn_v2:{isbn}");
            let body = if let Some(cached) = load_http_cache(db, "openlibrary", &cache_key)? {
                if cached.status_code >= 400 {
                    return Ok(Vec::new());
                }
                cached.body
            } else {
                let url = format!(
                    "{}/api/books?bibkeys=ISBN:{isbn}&format=json&jscmd=data",
                    self.base_url
                );
                let (status, text) = self.fetch_rate_limited(&url).await?;
                store_http_cache(db, "openlibrary", &cache_key, status, &text)?;
                if status >= 400 {
                    return Ok(Vec::new());
                }
                text
            };
            if let Ok(v) = serde_json::from_str::<Value>(&body) {
                if let Some(rec) = parse_openlibrary_isbn_any(&v, isbn) {
                    return Ok(vec![CandidateMatch {
                        score: 0.98,
                        source: "openlibrary".to_string(),
                        record: rec,
                    }]);
                }
            }
            return Ok(Vec::new());
        }

        let ti = ids
            .heuristic_title
            .as_deref()
            .map(str::trim)
            .unwrap_or("");
        let au = ids.heuristic_first_author.as_deref().unwrap_or("").trim();
        if ti.is_empty() && (!ids.is_user_query || au.is_empty()) {
            return Ok(Vec::new());
        }
        let cache_key = format!(
            "book_v2:ti={}:au={}",
            normalize_math_text(ti),
            normalize_math_text(au)
        );

        let body = if let Some(cached) = load_http_cache(db, "openlibrary", &cache_key)? {
            if cached.status_code >= 400 {
                return Ok(Vec::new());
            }
            cached.body
        } else {
            let clean_ti: String = ti
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let mut url = format!(
                "{}/search.json?limit=6&fields=key,title,subtitle,author_name,first_publish_year,publisher,publish_place,isbn,number_of_pages_median",
                self.base_url,
            );
            if !clean_ti.is_empty() {
                url.push_str("&title=");
                url.push_str(
                    &url::form_urlencoded::byte_serialize(clean_ti.as_bytes()).collect::<String>(),
                );
            }
            if !au.is_empty() {
                url.push_str("&author=");
                url.push_str(
                    &url::form_urlencoded::byte_serialize(au.as_bytes()).collect::<String>(),
                );
            }
            let (status, text) = self.fetch_rate_limited(&url).await?;
            store_http_cache(db, "openlibrary", &cache_key, status, &text)?;
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for doc in v
            .get("docs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(rec) = parse_openlibrary_search_doc(doc) {
                let score = score_candidate(&rec, ids);
                out.push(CandidateMatch {
                    score,
                    source: "openlibrary".to_string(),
                    record: rec,
                });
            }
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(out)
    }

    async fn fetch_rate_limited(&self, url: &str) -> Result<(u16, String)> {
        self.limiter.acquire().await?;
        let (status, text) = if let Some(mock) = &self.mock_transport {
            mock(url)
        } else {
            let resp = match self.http.get(url).send().await {
                Ok(r) => r,
                Err(e) => {
                    self.limiter.record_transient_failure(503).await;
                    return Err(CoreError::Network(format!("openlibrary request: {e}")));
                }
            };
            let status = resp.status().as_u16();
            let text = resp
                .text()
                .await
                .map_err(|e| CoreError::Network(format!("openlibrary read body: {e}")))?;
            (status, text)
        };

        if status == 429 || status >= 500 {
            self.limiter.record_transient_failure(status).await;
            return Err(CoreError::Network(format!("openlibrary HTTP {status}")));
        }

        self.limiter.record_success().await;
        Ok((status, text))
    }
}

fn parse_openlibrary_isbn_any(v: &Value, isbn: &str) -> Option<BibliographicRecord> {
    let bibkey = format!("ISBN:{isbn}");
    if let Some(entry) = v.get(&bibkey) {
        return parse_openlibrary_jscmd_data(entry, isbn);
    }
    if let Some(obj) = v.as_object() {
        for (k, entry) in obj {
            if k.starts_with("ISBN:") {
                if let Some(rec) = parse_openlibrary_jscmd_data(entry, isbn) {
                    return Some(rec);
                }
            }
        }
    }
    parse_openlibrary_isbn(v, isbn)
}

fn parse_openlibrary_jscmd_data(v: &Value, isbn: &str) -> Option<BibliographicRecord> {
    let main_title = v
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let subtitle = v
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

    let mut authors: Vec<BibliographicAuthor> = v
        .get("authors")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("name").and_then(Value::as_str))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(split_author_name)
                .collect()
        })
        .unwrap_or_default();
    if authors.is_empty() {
        if let Some(by) = v.get("by_statement").and_then(Value::as_str) {
            for part in by.split([',', ';']) {
                let trimmed = part.trim();
                if !trimmed.is_empty() {
                    authors.push(split_author_name(trimmed));
                }
            }
        }
    }

    let year = v
        .get("publish_date")
        .and_then(Value::as_str)
        .and_then(extract_4digit_year);
    let publisher = v
        .get("publishers")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|p| p.get("name").and_then(Value::as_str).or_else(|| p.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let address = v
        .get("publish_places")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|p| p.get("name").and_then(Value::as_str).or_else(|| p.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let pages = v
        .get("number_of_pages")
        .and_then(Value::as_u64)
        .filter(|&n| n > 0)
        .map(|n| n.to_string());
    let url = v
        .get("url")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| {
            v.get("key")
                .and_then(Value::as_str)
                .map(|k| format!("{OPENLIBRARY_BASE}{k}"))
        });

    Some(BibliographicRecord {
        work_type: WorkType::Book,
        title,
        authors,
        publisher,
        address,
        year,
        pages,
        isbn: Some(isbn.to_string()),
        url,
        source: "openlibrary".to_string(),
        ..Default::default()
    })
}

fn parse_openlibrary_isbn(v: &Value, isbn: &str) -> Option<BibliographicRecord> {
    let title = v
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();

    let mut authors = Vec::new();
    if let Some(by) = v.get("by_statement").and_then(Value::as_str) {
        for part in by.split([',', ';']) {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                authors.push(split_author_name(trimmed));
            }
        }
    }

    let year = v
        .get("publish_date")
        .and_then(Value::as_str)
        .and_then(extract_4digit_year);
    let publisher = v
        .get("publishers")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let pages = v
        .get("number_of_pages")
        .and_then(Value::as_u64)
        .map(|n| n.to_string());
    let key_path = v.get("key").and_then(Value::as_str);

    Some(BibliographicRecord {
        work_type: WorkType::Book,
        title,
        authors,
        publisher,
        year,
        pages,
        isbn: Some(isbn.to_string()),
        url: key_path.map(|k| format!("{OPENLIBRARY_BASE}{k}")),
        source: "openlibrary".to_string(),
        ..Default::default()
    })
}

fn parse_openlibrary_search_doc(v: &Value) -> Option<BibliographicRecord> {
    let main_title = v
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let subtitle = v
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
    let authors = v
        .get("author_name")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(split_author_name)
                .collect()
        })
        .unwrap_or_default();
    let year = v.get("first_publish_year").and_then(Value::as_i64);
    let publisher = v
        .get("publisher")
        .and_then(Value::as_array)
        .and_then(|a| {
            // Prefer well-known academic publishers over generic reprint labels when multiple editions exist
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .find(|s| !s.is_empty() && !s.to_ascii_lowercase().contains("reprint"))
                .or_else(|| a.first().and_then(Value::as_str).map(str::trim))
        })
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let address = v
        .get("publish_place")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().filter_map(Value::as_str).map(str::trim).find(|s| !s.is_empty()))
        .map(ToOwned::to_owned);
    let pages = v
        .get("number_of_pages_median")
        .and_then(Value::as_u64)
        .filter(|&n| n > 0)
        .map(|n| n.to_string());
    let isbn = v
        .get("isbn")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .find(|s| s.len() == 13)
                .or_else(|| a.first().and_then(Value::as_str))
        })
        .map(ToOwned::to_owned);
    let key_path = v.get("key").and_then(Value::as_str);

    Some(BibliographicRecord {
        work_type: WorkType::Book,
        title,
        authors,
        publisher,
        address,
        year,
        pages,
        isbn,
        url: key_path.map(|k| format!("{OPENLIBRARY_BASE}{k}")),
        source: "openlibrary".to_string(),
        ..Default::default()
    })
}

fn split_author_name(raw: &str) -> BibliographicAuthor {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() >= 2 {
        BibliographicAuthor {
            family_name: parts.last().unwrap_or(&"").to_string(),
            given_name: parts[..parts.len() - 1].join(" "),
            role: "author".to_string(),
        }
    } else {
        BibliographicAuthor {
            family_name: raw.trim().to_string(),
            given_name: String::new(),
            role: "author".to_string(),
        }
    }
}

fn extract_4digit_year(s: &str) -> Option<i64> {
    let re = regex::Regex::new(r"\b(1[89]\d{2}|20\d{2})\b").ok()?;
    re.captures(s).and_then(|c| c[1].parse::<i64>().ok())
}
