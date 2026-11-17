use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::metadata::extract::extract_prose_core_title;
use crate::metadata::limiter::{load_http_cache, store_http_cache, CrossrefRateLimiter};
use crate::metadata::models::{
    clean_author_surname, normalize_doi, normalize_pages, BibliographicAuthor, BibliographicRecord,
    CandidateMatch, ExtractedIdentifiers, WorkType,
};
use crate::metadata::mrlookup::score_candidate;
use crate::text_norm::normalize_math_text;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;
use url::form_urlencoded;

const CROSSREF_API: &str = "https://api.crossref.org/works";
const DOI_ORG_BASE: &str = "https://doi.org";

#[derive(Debug)]
pub struct CrossrefClient {
    http: Client,
    polite_email: Option<String>,
    single_limiter: CrossrefRateLimiter,
    list_limiter: CrossrefRateLimiter,
}

impl CrossrefClient {
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
            .map_err(|e| CoreError::Network(format!("crossref client init: {e}")))?;

        let has_mail = cleaned_email.is_some();
        Ok(Self {
            http,
            polite_email: cleaned_email,
            single_limiter: CrossrefRateLimiter::new(has_mail, false),
            list_limiter: CrossrefRateLimiter::new(has_mail, true),
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
            if let Some(hit) = self.lookup_exact_doi(db, doi).await? {
                return Ok(vec![hit]);
            }
            if let Some(hit) = self.lookup_doi_content_negotiation(db, doi).await? {
                return Ok(vec![hit]);
            }
        }

        if let Some(pii) = ids.pii.as_deref() {
            if let Some(hit) = self.lookup_alternative_id(db, pii).await? {
                return Ok(vec![hit]);
            }
        }

        if let Some(isbn) = ids.isbn.as_deref() {
            if let Some(hit) = self.lookup_isbn(db, isbn).await? {
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
            let cache_q = if surname.is_empty() {
                ti.to_string()
            } else {
                format!("{ti} {surname}")
            };
            let ck = format!("q:{}", normalize_math_text(&cache_q));
            let (bib_q, au_filter, rows_str) = if surname.is_empty() {
                (ti, None, "15")
            } else {
                (ti, Some(surname.as_str()), "10")
            };
            let mut hits = self
                .search_list(db, bib_q, au_filter, None, rows_str, &ck, false, ids)
                .await?;

            // When searching by title alone without an author, `query.bibliographic` matches across
            // abstracts and conference names (often burying IEEE proceedings below unrelated hits).
            // Query Crossref's `query.title` index as well if bibliographic search missed strong authored hits.
            let has_strong_authored_hit = hits
                .iter()
                .any(|h| h.score >= 0.88 && !h.record.authors.is_empty());
            if surname.is_empty() && !has_strong_authored_hit {
                let qt_ck = format!("qt:{}", normalize_math_text(ti));
                if let Ok(qt_hits) = self
                    .search_list(db, ti, None, None, "15", &qt_ck, true, ids)
                    .await
                {
                    merge_crossref_hits(&mut hits, qt_hits);
                }
            }

            let mut top_score = hits.first().map(|c| c.score).unwrap_or(0.0);
            let ti_words = normalize_math_text(ti).split_whitespace().count();
            if !surname.is_empty() && ti_words >= 4 && top_score < 0.58 {
                let fb_ck = format!("q:{}", normalize_math_text(ti));
                let mut ids_no_au = ids.clone();
                ids_no_au.heuristic_first_author = None;
                ids_no_au.heuristic_authors.clear();
                let mut fb_hits = self
                    .search_list(db, ti, None, None, "15", &fb_ck, false, &ids_no_au)
                    .await?;
                if !fb_hits
                    .iter()
                    .any(|h| h.score >= 0.88 && !h.record.authors.is_empty())
                {
                    let qt_ck = format!("qt:{}", normalize_math_text(ti));
                    if let Ok(qt_hits) = self
                        .search_list(db, ti, None, None, "15", &qt_ck, true, &ids_no_au)
                        .await
                    {
                        merge_crossref_hits(&mut fb_hits, qt_hits);
                    }
                }
                for h in &mut fb_hits {
                    h.score *= 0.97;
                }
                if fb_hits.first().map(|c| c.score).unwrap_or(0.0) > top_score {
                    hits = fb_hits;
                    top_score = hits.first().map(|c| c.score).unwrap_or(0.0);
                }
            }

            // If mangled math formulas in `ti` degraded Crossref ranking, retry with clean prose core
            if top_score < 0.58 {
                if let Some(prose_ti) = extract_prose_core_title(ti) {
                    let p_ck = format!("qp:{}", normalize_math_text(&prose_ti));
                    let p_hits = self
                        .search_list(db, &prose_ti, au_filter, None, "12", &p_ck, false, ids)
                        .await?;
                    if p_hits.first().map(|c| c.score).unwrap_or(0.0) > top_score {
                        hits = p_hits;
                    }
                }
            }

            return Ok(hits);
        }

        if !surname.is_empty() {
            let kw = ids.filename_keywords.join(" ");
            let (q, yf) = match (ids.heuristic_year, kw.is_empty()) {
                (Some(yr), true) => (
                    format!("{surname} {yr}"),
                    Some(format!("from-pub-date:{},until-pub-date:{}", yr - 1, yr + 1)),
                ),
                (Some(yr), false) => (
                    format!("{surname} {yr} {kw}"),
                    Some(format!("from-pub-date:{},until-pub-date:{}", yr - 1, yr + 1)),
                ),
                (None, true) => (surname.clone(), None),
                (None, false) => (format!("{surname} {kw}"), None),
            };
            let ck = format!("auyr:{}", normalize_math_text(&q));
            return self
                .search_list(db, &q, Some(&surname), yf.as_deref(), "10", &ck, false, ids)
                .await;
        }

        Ok(Vec::new())
    }

    pub async fn lookup_alternative_id(
        &self,
        db: &Database,
        pii: &str,
    ) -> Result<Option<CandidateMatch>> {
        let clean_pii = pii.trim();
        if clean_pii.is_empty() {
            return Ok(None);
        }
        let cache_key = format!("pii:{}", clean_pii.to_ascii_uppercase());
        let body = if let Some(c) = load_http_cache(db, "crossref", &cache_key)? {
            if c.status_code >= 400 {
                return Ok(None);
            }
            c.body
        } else {
            self.list_limiter.acquire().await;
            let filter_val = format!("alternative-id:{clean_pii}");
            let mut query_pairs = vec![("filter", filter_val.as_str()), ("rows", "2")];
            if let Some(mail) = &self.polite_email {
                query_pairs.push(("mailto", mail.as_str()));
            }
            let req_url = url::Url::parse_with_params(CROSSREF_API, &query_pairs)
                .map_err(|e| CoreError::Network(format!("crossref pii url: {e}")))?;
            let resp = self
                .http
                .get(req_url)
                .send()
                .await
                .map_err(|e| CoreError::Network(format!("crossref pii request: {e}")))?;
            let status = resp.status().as_u16();
            let text = resp
                .text()
                .await
                .map_err(|e| CoreError::Network(format!("crossref pii read: {e}")))?;
            if status < 400 || status == 404 {
                store_http_cache(db, "crossref", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(None);
            }
            text
        };

        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            return Ok(None);
        };
        if let Some(item) = v
            .get("message")
            .and_then(|m| m.get("items"))
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
        {
            if let Some(rec) = parse_crossref_work(item) {
                return Ok(Some(CandidateMatch {
                    score: 0.99,
                    source: "crossref".to_string(),
                    record: rec,
                }));
            }
        }
        Ok(None)
    }

    /// Resolves a monograph/book by normalized ISBN via Crossref `filter=isbn:<isbn>`.
    /// Prefers full book/monograph records over individual book-chapter records sharing the ISBN.
    pub async fn lookup_isbn(
        &self,
        db: &Database,
        isbn: &str,
    ) -> Result<Option<CandidateMatch>> {
        let clean_isbn: String = isbn
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x')
            .map(|c| c.to_ascii_uppercase())
            .collect();
        if clean_isbn.len() != 10 && clean_isbn.len() != 13 {
            return Ok(None);
        }
        let cache_key = format!("isbn:{clean_isbn}");
        let body = if let Some(c) = load_http_cache(db, "crossref", &cache_key)? {
            if c.status_code >= 400 {
                return Ok(None);
            }
            c.body
        } else {
            self.list_limiter.acquire().await;
            let filter_val = format!("isbn:{clean_isbn}");
            let mut query_pairs = vec![("filter", filter_val.as_str()), ("rows", "5")];
            if let Some(mail) = &self.polite_email {
                query_pairs.push(("mailto", mail.as_str()));
            }
            let req_url = url::Url::parse_with_params(CROSSREF_API, &query_pairs)
                .map_err(|e| CoreError::Network(format!("crossref isbn url: {e}")))?;
            let Ok(resp) = self.http.get(req_url).send().await else {
                return Ok(None);
            };
            let hdrs = resp.headers().clone();
            let limit_h = hdrs.get("x-rate-limit-limit").and_then(|v| v.to_str().ok());
            let int_h = hdrs.get("x-rate-limit-interval").and_then(|v| v.to_str().ok());
            let type_h = hdrs.get("x-rate-limit-type").and_then(|v| v.to_str().ok());
            self.list_limiter
                .adapt_from_headers(limit_h, int_h, type_h)
                .await;
            let status = resp.status().as_u16();
            let Ok(text) = resp.text().await else {
                return Ok(None);
            };
            if status < 400 || status == 404 {
                store_http_cache(db, "crossref", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(None);
            }
            text
        };

        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            return Ok(None);
        };
        let Some(items) = v
            .get("message")
            .and_then(|m| m.get("items"))
            .and_then(Value::as_array)
        else {
            return Ok(None);
        };

        let mut parsed_books: Vec<BibliographicRecord> = items
            .iter()
            .filter_map(parse_crossref_work)
            .filter(|rec| rec.work_type == WorkType::Book)
            .collect();
        // Prefer book record with explicit authors over editor-only/anonymous metadata if multiple exist
        parsed_books.sort_by_key(|r| std::cmp::Reverse(r.authors.len()));
        if let Some(mut rec) = parsed_books.into_iter().next() {
            if rec.isbn.is_none() {
                rec.isbn = Some(clean_isbn);
            }
            return Ok(Some(CandidateMatch {
                score: 0.99,
                source: "crossref".to_string(),
                record: rec,
            }));
        }
        Ok(None)
    }

    /// Official International DOI Foundation (`doi.org`) HTTP content negotiation in CSL-JSON format.
    /// Resolves DOIs registered with ANY agency (DataCite, mEDRA, ETH, CERN/Zenodo, HAL, JaLC, Crossref).
    pub async fn lookup_doi_content_negotiation(
        &self,
        db: &Database,
        doi: &str,
    ) -> Result<Option<CandidateMatch>> {
        if doi.starts_with("10.48550/") {
            return Ok(None);
        }
        let cache_key = format!("doi:{doi}");
        let body = if let Some(c) = load_http_cache(db, "doiorg", &cache_key)? {
            if c.status_code >= 400 {
                return Ok(None);
            }
            c.body
        } else {
            self.single_limiter.acquire().await;
            let encoded = doi
                .split('/')
                .map(|seg| form_urlencoded::byte_serialize(seg.as_bytes()).collect::<String>())
                .collect::<Vec<_>>()
                .join("/");
            let url = format!("{DOI_ORG_BASE}/{encoded}");
            let Ok(resp) = self
                .http
                .get(&url)
                .header("Accept", "application/vnd.citationstyles.csl+json")
                .send()
                .await
            else {
                return Ok(None);
            };
            let status = resp.status().as_u16();
            let Ok(text) = resp.text().await else {
                return Ok(None);
            };
            if status < 400 || status == 404 {
                store_http_cache(db, "doiorg", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(None);
            }
            text
        };

        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            return Ok(None);
        };
        if let Some(rec) = parse_csl_json_work(&v) {
            return Ok(Some(CandidateMatch {
                score: 0.98,
                source: "crossref".to_string(),
                record: rec,
            }));
        }
        Ok(None)
    }

    pub async fn lookup_exact_doi(
        &self,
        db: &Database,
        doi: &str,
    ) -> Result<Option<CandidateMatch>> {
        let encoded: String = form_urlencoded::byte_serialize(doi.as_bytes()).collect();
        let cache_key = format!("doi:{doi}");
        let body = if let Some(c) = load_http_cache(db, "crossref", &cache_key)? {
            if c.status_code >= 400 {
                return Ok(None);
            }
            c.body
        } else {
            self.single_limiter.acquire().await;
            let url = if let Some(mail) = &self.polite_email {
                let m_enc: String = form_urlencoded::byte_serialize(mail.as_bytes()).collect();
                format!("{CROSSREF_API}/{encoded}?mailto={m_enc}")
            } else {
                format!("{CROSSREF_API}/{encoded}")
            };
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .map_err(|e| CoreError::Network(format!("crossref doi request: {e}")))?;
            let hdrs = resp.headers().clone();
            let limit_h = hdrs.get("x-rate-limit-limit").and_then(|v| v.to_str().ok());
            let int_h = hdrs.get("x-rate-limit-interval").and_then(|v| v.to_str().ok());
            let type_h = hdrs.get("x-rate-limit-type").and_then(|v| v.to_str().ok());
            self.single_limiter
                .adapt_from_headers(limit_h, int_h, type_h)
                .await;

            let status = resp.status().as_u16();
            let text = resp
                .text()
                .await
                .map_err(|e| CoreError::Network(format!("crossref read: {e}")))?;
            if status < 400 || status == 404 {
                store_http_cache(db, "crossref", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(None);
            }
            text
        };
        let Ok(v) = serde_json::from_str::<Value>(&body) else {
            return Ok(None);
        };
        if let Some(msg) = v.get("message") {
            if let Some(rec) = parse_crossref_work(msg) {
                return Ok(Some(CandidateMatch {
                    score: 0.99,
                    source: "crossref".to_string(),
                    record: rec,
                }));
            }
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    async fn search_list(
        &self,
        db: &Database,
        query_str: &str,
        author_filter: Option<&str>,
        year_filter: Option<&str>,
        rows_str: &str,
        cache_key: &str,
        title_field_only: bool,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        let body = if let Some(c) = load_http_cache(db, "crossref", cache_key)? {
            if c.status_code >= 400 {
                return Ok(Vec::new());
            }
            c.body
        } else {
            self.list_limiter.acquire().await;
            let q_param = if title_field_only {
                "query.title"
            } else {
                "query.bibliographic"
            };
            let mut query_pairs = vec![
                (q_param, query_str),
                ("rows", rows_str),
            ];
            if let Some(af) = author_filter {
                query_pairs.push(("query.author", af));
            }
            if let Some(yf) = year_filter {
                query_pairs.push(("filter", yf));
            }
            if let Some(mail) = &self.polite_email {
                query_pairs.push(("mailto", mail.as_str()));
            }
            let req_url = url::Url::parse_with_params(CROSSREF_API, &query_pairs)
                .map_err(|e| CoreError::Network(format!("crossref list url: {e}")))?;
            let resp = self
                .http
                .get(req_url)
                .send()
                .await
                .map_err(|e| CoreError::Network(format!("crossref list request: {e}")))?;
            let hdrs = resp.headers().clone();
            let limit_h = hdrs.get("x-rate-limit-limit").and_then(|v| v.to_str().ok());
            let int_h = hdrs.get("x-rate-limit-interval").and_then(|v| v.to_str().ok());
            let type_h = hdrs.get("x-rate-limit-type").and_then(|v| v.to_str().ok());
            self.list_limiter
                .adapt_from_headers(limit_h, int_h, type_h)
                .await;

            let status = resp.status().as_u16();
            let text = resp
                .text()
                .await
                .map_err(|e| CoreError::Network(format!("crossref list read: {e}")))?;
            if status < 400 || status == 404 {
                store_http_cache(db, "crossref", cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        let v: Value = serde_json::from_str(&body)
            .map_err(|e| CoreError::Network(format!("crossref list json: {e}")))?;
        let mut out = Vec::new();
        if let Some(items) = v
            .get("message")
            .and_then(|m| m.get("items"))
            .and_then(Value::as_array)
        {
            for item in items {
                if let Some(rec) = parse_crossref_work(item) {
                    let score = score_candidate(&rec, ids);
                    out.push(CandidateMatch {
                        score,
                        source: "crossref".to_string(),
                        record: rec,
                    });
                }
            }
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(out)
    }
}

fn merge_crossref_hits(dest: &mut Vec<CandidateMatch>, extra: Vec<CandidateMatch>) {
    for cand in extra {
        let already = dest.iter().any(|existing| {
            if let (Some(da), Some(db)) =
                (existing.record.doi.as_deref(), cand.record.doi.as_deref())
            {
                if !da.is_empty() && da.eq_ignore_ascii_case(db) {
                    return true;
                }
            }
            existing.record.title.eq_ignore_ascii_case(&cand.record.title)
                && existing.record.year == cand.record.year
                && existing.record.authors.len() == cand.record.authors.len()
        });
        if !already {
            dest.push(cand);
        }
    }
    dest.sort_by(|a, b| b.score.total_cmp(&a.score));
}

pub fn parse_crossref_work(w: &Value) -> Option<BibliographicRecord> {
    let title = w
        .get("title")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();

    let cr_type = w.get("type").and_then(Value::as_str).unwrap_or("journal-article");
    let work_type = match cr_type {
        "book" | "monograph" | "edited-book" => WorkType::Book,
        "book-chapter" => WorkType::Incollection,
        "proceedings-article" => WorkType::Inproceedings,
        "dissertation" => WorkType::Phdthesis,
        _ => WorkType::Article,
    };

    let mut authors = Vec::new();
    for a in w.get("author").and_then(Value::as_array).into_iter().flatten() {
        let fam = a.get("family").and_then(Value::as_str).unwrap_or("").trim();
        let giv = a.get("given").and_then(Value::as_str).unwrap_or("").trim();
        if !fam.is_empty() || !giv.is_empty() {
            authors.push(BibliographicAuthor {
                family_name: fam.to_string(),
                given_name: giv.to_string(),
                role: "author".to_string(),
            });
        }
    }
    for e in w.get("editor").and_then(Value::as_array).into_iter().flatten() {
        let fam = e.get("family").and_then(Value::as_str).unwrap_or("").trim();
        let giv = e.get("given").and_then(Value::as_str).unwrap_or("").trim();
        if !fam.is_empty() || !giv.is_empty() {
            authors.push(BibliographicAuthor {
                family_name: fam.to_string(),
                given_name: giv.to_string(),
                role: "editor".to_string(),
            });
        }
    }

    let container_full = w
        .get("container-title")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let container_short = w
        .get("short-container-title")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let (journal_full, journal_abbrev, booktitle) = match work_type {
        WorkType::Incollection | WorkType::Inproceedings => (None, None, container_full),
        _ => (container_full, container_short, None),
    };

    let year = ["published-print", "published-online", "issued", "created"]
        .iter()
        .find_map(|k| {
            w.get(*k)
                .and_then(|d| d.get("date-parts"))
                .and_then(Value::as_array)
                .and_then(|outer| outer.first())
                .and_then(Value::as_array)
                .and_then(|parts| parts.first())
                .and_then(Value::as_i64)
        });

    let doi = w.get("DOI").and_then(Value::as_str).and_then(normalize_doi);
    let url = w
        .get("URL")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| doi.as_ref().map(|d| format!("https://doi.org/{d}")));

    Some(BibliographicRecord {
        work_type,
        title,
        authors,
        journal_abbrev,
        journal_full,
        booktitle,
        series: None,
        volume: w.get("volume").and_then(Value::as_str).map(ToOwned::to_owned),
        issue_number: w.get("issue").and_then(Value::as_str).map(ToOwned::to_owned),
        year,
        pages: w.get("page").and_then(Value::as_str).map(normalize_pages),
        edition: w.get("edition-number").and_then(Value::as_str).map(ToOwned::to_owned),
        note: None,
        publisher: w.get("publisher").and_then(Value::as_str).map(ToOwned::to_owned),
        address: w
            .get("publisher-location")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        isbn: w
            .get("ISBN")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        issn: w
            .get("ISSN")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        coden: None,
        doi,
        arxiv_id: None,
        arxiv_version: None,
        mr_number: None,
        mr_number_display: None,
        mr_reviewer: None,
        zbl_number: None,
        url,
        msc_primary: None,
        msc_secondary: Vec::new(),
        abstract_text: w.get("abstract").and_then(Value::as_str).map(strip_jats_tags),
        raw_mrlookup_bibtex: None,
        source: "crossref".to_string(),
        license_note: None,
    })
}

fn json_str_or_first(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_string())
        }
        Some(Value::Array(arr)) => arr
            .first()
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned),
        _ => None,
    }
}

/// Parses standard CSL-JSON (`application/vnd.citationstyles.csl+json`) returned by `https://doi.org/<doi>`.
pub fn parse_csl_json_work(w: &Value) -> Option<BibliographicRecord> {
    let title = json_str_or_first(w.get("title"))?;
    let csl_type = w
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("article-journal");
    let work_type = match csl_type {
        "book" | "monograph" | "edited-book" => WorkType::Book,
        "chapter" | "book-chapter" => WorkType::Incollection,
        "paper-conference" | "proceedings-article" => WorkType::Inproceedings,
        "thesis" | "dissertation" => WorkType::Phdthesis,
        "report" | "dataset" => WorkType::Misc,
        _ => WorkType::Article,
    };

    let mut authors = Vec::new();
    for a in w.get("author").and_then(Value::as_array).into_iter().flatten() {
        let fam = a
            .get("family")
            .or_else(|| a.get("literal"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let giv = a.get("given").and_then(Value::as_str).unwrap_or("").trim();
        if !fam.is_empty() || !giv.is_empty() {
            authors.push(BibliographicAuthor {
                family_name: fam.to_string(),
                given_name: giv.to_string(),
                role: "author".to_string(),
            });
        }
    }
    for e in w.get("editor").and_then(Value::as_array).into_iter().flatten() {
        let fam = e
            .get("family")
            .or_else(|| e.get("literal"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let giv = e.get("given").and_then(Value::as_str).unwrap_or("").trim();
        if !fam.is_empty() || !giv.is_empty() {
            authors.push(BibliographicAuthor {
                family_name: fam.to_string(),
                given_name: giv.to_string(),
                role: "editor".to_string(),
            });
        }
    }

    let container_full = json_str_or_first(w.get("container-title"));
    let container_short = json_str_or_first(
        w.get("container-title-short")
            .or_else(|| w.get("short-container-title")),
    );
    let (journal_full, journal_abbrev, booktitle) = match work_type {
        WorkType::Incollection | WorkType::Inproceedings => (None, None, container_full),
        _ => (container_full, container_short, None),
    };

    let year = ["issued", "published-print", "published-online", "created"]
        .iter()
        .find_map(|k| {
            w.get(*k)
                .and_then(|d| d.get("date-parts"))
                .and_then(Value::as_array)
                .and_then(|outer| outer.first())
                .and_then(Value::as_array)
                .and_then(|parts| parts.first())
                .and_then(|p| p.as_i64().or_else(|| p.as_str()?.parse::<i64>().ok()))
        });

    let doi = w
        .get("DOI")
        .or_else(|| w.get("doi"))
        .and_then(Value::as_str)
        .and_then(normalize_doi);
    let url = w
        .get("URL")
        .or_else(|| w.get("url"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| doi.as_ref().map(|d| format!("https://doi.org/{d}")));

    Some(BibliographicRecord {
        work_type,
        title,
        authors,
        journal_abbrev,
        journal_full,
        booktitle,
        series: json_str_or_first(w.get("collection-title")),
        volume: json_str_or_first(w.get("volume")),
        issue_number: json_str_or_first(w.get("issue")),
        year,
        pages: json_str_or_first(w.get("page")).map(|p| normalize_pages(&p)),
        edition: json_str_or_first(w.get("edition")),
        note: None,
        publisher: json_str_or_first(w.get("publisher")),
        address: json_str_or_first(w.get("publisher-place").or_else(|| w.get("publisher-location"))),
        isbn: json_str_or_first(w.get("ISBN")),
        issn: json_str_or_first(w.get("ISSN")),
        coden: None,
        doi,
        arxiv_id: None,
        arxiv_version: None,
        mr_number: None,
        mr_number_display: None,
        mr_reviewer: None,
        zbl_number: None,
        url,
        msc_primary: None,
        msc_secondary: Vec::new(),
        abstract_text: w.get("abstract").and_then(Value::as_str).map(strip_jats_tags),
        raw_mrlookup_bibtex: None,
        source: "crossref".to_string(),
        license_note: Some("DOI Content Negotiation (doi.org)".to_string()),
    })
}

fn strip_jats_tags(raw: &str) -> String {
    let re = regex::Regex::new(r"<[^>]+>").unwrap();
    re.replace_all(raw, " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
