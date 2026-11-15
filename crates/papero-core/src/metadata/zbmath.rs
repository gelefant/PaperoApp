use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::metadata::limiter::{load_http_cache, store_http_cache, SequentialRateLimiter};
use crate::metadata::models::{
    clean_author_surname, normalize_doi, normalize_pages, BibliographicAuthor, BibliographicRecord,
    CandidateMatch, ExtractedIdentifiers, WorkType,
};
use crate::metadata::mrlookup::score_candidate;
use crate::text_norm::normalize_math_text;
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

use crate::metadata::journal_abbrev::{looks_abbreviated_journal, serial_titles_match};

const ZBMATH_SEARCH_URL: &str = "https://api.zbmath.org/v1/document/_search";
const ZBMATH_SERIAL_SEARCH_URL: &str = "https://api.zbmath.org/v1/serial/_search";
const ZBMATH_LICENSE_NOTE: &str = "CC-BY-SA 4.0 (zbMATH Open / FIZ Karlsruhe)";

#[derive(Debug)]
pub struct ZbMathClient {
    http: Client,
    limiter: SequentialRateLimiter,
    search_url: String,
    serial_search_url: String,
}

impl ZbMathClient {
    pub fn new(user_agent: String) -> Result<Self> {
        Self::with_urls(
            user_agent,
            ZBMATH_SEARCH_URL.to_string(),
            ZBMATH_SERIAL_SEARCH_URL.to_string(),
        )
    }

    fn with_urls(
        user_agent: String,
        search_url: String,
        serial_search_url: String,
    ) -> Result<Self> {
        let http = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| CoreError::Network(format!("zbmath client init: {e}")))?;
        Ok(Self {
            http,
            limiter: SequentialRateLimiter::new(Duration::from_secs(1)),
            search_url,
            serial_search_url,
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

        if let Some(zbl) = ids.zbl_number.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(body) = self
                .fetch_zbmath_raw(db, &format!("an:{zbl}"), &format!("zbl:{zbl}"))
                .await?
            {
                let hits = parse_zbmath_response(&body, ids)?;
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
                .fetch_zbmath_raw(db, &format!("do:{doi}"), &format!("doi:{doi}"))
                .await?
            {
                let hits = parse_zbmath_response(&body, ids)?;
                if !hits.is_empty() {
                    return Ok(hits);
                }
            }
        }

        if let Some(ti) = ids
            .heuristic_title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let raw_q = if surname.is_empty() {
                ti.to_string()
            } else {
                format!("{ti} {surname}")
            };
            let cache_key = format!("q:{}", normalize_math_text(&raw_q));
            let search_string = build_zbmath_title_query(ti, &surname);

            let mut hits = if let Some(body) = self
                .fetch_zbmath_raw(db, &search_string, &cache_key)
                .await?
            {
                parse_zbmath_response(&body, ids)?
            } else {
                Vec::new()
            };

            let mut top_score = hits.first().map(|c| c.score).unwrap_or(0.0);
            let ti_tokens = zbmath_title_tokens(ti);
            if !surname.is_empty() && top_score < 0.55 && ti_tokens.len() >= 3 {
                let fb_cache_key = format!("q:{}", normalize_math_text(ti));
                let fb_search = build_zbmath_title_query(ti, "");
                if let Some(fb_body) = self
                    .fetch_zbmath_raw(db, &fb_search, &fb_cache_key)
                    .await?
                {
                    let mut ids_no_au = ids.clone();
                    ids_no_au.heuristic_first_author = None;
                    ids_no_au.heuristic_authors.clear();
                    let mut fb_hits = parse_zbmath_response(&fb_body, &ids_no_au)?;
                    for h in &mut fb_hits {
                        h.score *= 0.97;
                    }
                    if fb_hits.first().map(|c| c.score).unwrap_or(0.0) > top_score {
                        hits = fb_hits;
                        top_score = hits.first().map(|c| c.score).unwrap_or(0.0);
                    }
                }
            }

            // Relaxed core-5-longest-words retry when 1 noisy/OCR token in a long title caused
            // zbMATH's strict boolean `AND` to return 0 results.
            if top_score < 0.55 && ti_tokens.len() >= 5 {
                if let Some((core_search, core_key_words)) =
                    build_zbmath_core_title_query(&ti_tokens, &surname)
                {
                    let core_ck = format!("qcore:{core_key_words}");
                    if let Some(core_body) = self
                        .fetch_zbmath_raw(db, &core_search, &core_ck)
                        .await?
                    {
                        let core_hits = parse_zbmath_response(&core_body, ids)?;
                        if core_hits.first().map(|c| c.score).unwrap_or(0.0) > top_score {
                            hits = core_hits;
                        }
                    }
                }
            }
            return Ok(hits);
        }

        if !surname.is_empty() {
            let au_clause = format_zbmath_author_clause(&surname);
            let kw_tokens: Vec<String> = ids
                .filename_keywords
                .iter()
                .flat_map(|k| zbmath_title_tokens(k))
                .take(4)
                .map(|w| format!("ti:{w}"))
                .collect();
            let mut parts = vec![au_clause];
            if let Some(yr) = ids.heuristic_year {
                parts.push(format!("py:{yr}"));
            }
            parts.extend(kw_tokens);
            let search_string = parts.join(" and ");
            let raw_key_q = match (ids.heuristic_year, ids.filename_keywords.is_empty()) {
                (Some(yr), true) => format!("au:{surname} py:{yr}"),
                (Some(yr), false) => {
                    format!("au:{surname} py:{yr} {}", ids.filename_keywords.join(" "))
                }
                (None, true) => format!("au:{surname}"),
                (None, false) => format!("au:{surname} {}", ids.filename_keywords.join(" ")),
            };
            let cache_key = format!("auyr:{}", normalize_math_text(&raw_key_q));
            if let Some(body) = self
                .fetch_zbmath_raw(db, &search_string, &cache_key)
                .await?
            {
                return parse_zbmath_response(&body, ids);
            }
        }

        Ok(Vec::new())
    }

    pub async fn lookup_serial_abbrev(
        &self,
        db: &Database,
        journal_name: Option<&str>,
        issn: Option<&str>,
    ) -> Result<Option<String>> {
        let clean_name = journal_name.map(str::trim).filter(|s| !s.is_empty());
        let clean_issn = issn
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_uppercase());

        if let Some(ref sn) = clean_issn {
            let ck = format!("sn:{sn}");
            if let Some(body) = self
                .fetch_cached_get(
                    db,
                    "zbmath_serial",
                    &self.serial_search_url,
                    sn,
                    "5",
                    &ck,
                )
                .await?
            {
                if let Some(abbrev) = parse_zbmath_serial_abbrev(&body, clean_name, Some(sn))? {
                    return Ok(Some(abbrev));
                }
            }
        }

        if let Some(name) = clean_name {
            let norm = normalize_math_text(name);
            if norm.len() >= 3 {
                let ck = format!("ti:{norm}");
                if let Some(body) = self
                    .fetch_cached_get(
                        db,
                        "zbmath_serial",
                        &self.serial_search_url,
                        name,
                        "5",
                        &ck,
                    )
                    .await?
                {
                    if let Some(abbrev) =
                        parse_zbmath_serial_abbrev(&body, Some(name), clean_issn.as_deref())?
                    {
                        return Ok(Some(abbrev));
                    }
                }
            }
        }

        Ok(None)
    }

    async fn fetch_zbmath_raw(
        &self,
        db: &Database,
        search_string: &str,
        cache_key: &str,
    ) -> Result<Option<String>> {
        self.fetch_cached_get(
            db,
            "zbmath",
            &self.search_url,
            search_string,
            "10",
            cache_key,
        )
        .await
    }

    async fn fetch_cached_get(
        &self,
        db: &Database,
        cache_source: &str,
        base_url: &str,
        search_string: &str,
        results_per_page: &str,
        cache_key: &str,
    ) -> Result<Option<String>> {
        if let Some(cached) = load_http_cache(db, cache_source, cache_key)? {
            if cached.status_code >= 400 {
                return Ok(None);
            }
            return Ok(Some(cached.body));
        }

        self.limiter.acquire().await;
        let req_url = url::Url::parse_with_params(
            base_url,
            &[
                ("search_string", search_string),
                ("results_per_page", results_per_page),
            ],
        )
        .map_err(|e| CoreError::Network(format!("zbmath url build: {e}")))?;
        let resp = self
            .http
            .get(req_url)
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("zbmath request: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| CoreError::Network(format!("zbmath read body: {e}")))?;
        if status < 400 || status == 404 || status == 422 {
            store_http_cache(db, cache_source, cache_key, status, &text)?;
        }
        if status >= 400 {
            return Ok(None);
        }
        Ok(Some(text))
    }
}

fn zbmath_title_tokens(raw: &str) -> Vec<String> {
    let mut seen = Vec::<String>::new();
    for tok in normalize_math_text(raw).split(|c: char| c.is_whitespace() || c == '-') {
        if tok.len() < 3 || !tok.chars().all(|c| c.is_alphabetic()) {
            continue;
        }
        if matches!(
            tok,
            "and"
                | "not"
                | "the"
                | "for"
                | "with"
                | "from"
                | "into"
                | "over"
                | "under"
                | "via"
                | "some"
                | "sur"
                | "les"
                | "des"
                | "une"
                | "dans"
                | "und"
                | "der"
                | "die"
                | "das"
                | "del"
                | "con"
                | "una"
        ) {
            continue;
        }
        if !seen.iter().any(|w| w == tok) {
            seen.push(tok.to_string());
        }
        if seen.len() >= 9 {
            break;
        }
    }
    seen
}

fn build_zbmath_core_title_query(tokens: &[String], surname: &str) -> Option<(String, String)> {
    let mut ranked: Vec<&String> = tokens.iter().filter(|w| w.len() >= 4).collect();
    if ranked.len() < 4 {
        return None;
    }
    ranked.sort_by_key(|w| std::cmp::Reverse(w.len()));
    ranked.truncate(5);
    let mut parts: Vec<String> = ranked.iter().map(|w| format!("ti:{w}")).collect();
    let mut key_words = ranked
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if !surname.trim().is_empty() {
        let au = format_zbmath_author_clause(surname);
        if !au.is_empty() {
            parts.push(au);
            key_words.push(' ');
            key_words.push_str(&normalize_math_text(surname));
        }
    }
    Some((parts.join(" and "), key_words))
}

fn format_zbmath_author_clause(surname: &str) -> String {
    let norm = normalize_math_text(surname);
    let clean = norm.trim();
    if clean.contains(' ') {
        format!("au:\"{clean}\"")
    } else if !clean.is_empty() {
        format!("au:{clean}")
    } else {
        String::new()
    }
}

pub(crate) fn build_zbmath_title_query(ti: &str, surname: &str) -> String {
    let tokens = zbmath_title_tokens(ti);
    let mut parts: Vec<String> = if !tokens.is_empty() {
        tokens.into_iter().map(|w| format!("ti:{w}")).collect()
    } else {
        let fallback = normalize_math_text(ti);
        if fallback.is_empty() {
            Vec::new()
        } else {
            vec![format!("ti:\"{fallback}\"")]
        }
    };
    if !surname.trim().is_empty() {
        let au = format_zbmath_author_clause(surname);
        if !au.is_empty() {
            parts.push(au);
        }
    }
    parts.join(" and ")
}

pub fn parse_zbmath_response(body: &str, ids: &ExtractedIdentifiers) -> Result<Vec<CandidateMatch>> {
    let val: Value = serde_json::from_str(body)
        .map_err(|e| CoreError::Network(format!("zbmath json parse: {e}")))?;
    let Some(arr) = val.get("result").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for item in arr {
        let Some(rec) = parse_zbmath_item(item) else {
            continue;
        };
        let score = score_candidate(&rec, ids);
        out.push(CandidateMatch {
            score,
            source: "zbmath".to_string(),
            record: rec,
        });
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

fn parse_zbmath_item(item: &Value) -> Option<BibliographicRecord> {
    let title = item
        .get("title")
        .and_then(|t| t.get("title"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();

    let mut authors = Vec::new();
    if let Some(contribs) = item.get("contributors") {
        for a in contribs
            .get("authors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = a.get("name").and_then(Value::as_str) {
                authors.push(parse_comma_author(name, "author"));
            }
        }
        for e in contribs
            .get("editors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = e.get("name").and_then(Value::as_str) {
                authors.push(parse_comma_author(name, "editor"));
            }
        }
    }

    let doc_code = item
        .get("document_type")
        .and_then(|d| d.get("code"))
        .and_then(Value::as_str)
        .unwrap_or("j");
    let work_type = match doc_code {
        "b" => WorkType::Book,
        "a" => WorkType::Incollection,
        "p" => WorkType::Inproceedings,
        _ => WorkType::Article,
    };

    let source_obj = item.get("source");
    let series_first = source_obj
        .and_then(|s| s.get("series"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.first());
    let book_first = source_obj
        .and_then(|s| s.get("book"))
        .and_then(Value::as_array)
        .and_then(|arr| arr.first());

    let pages = source_obj
        .and_then(|s| s.get("pages"))
        .and_then(Value::as_str)
        .map(normalize_pages);

    let (journal_full, journal_abbrev, volume, issue_number, publisher, issn) =
        if let Some(ser) = series_first {
            let issn_val = ser
                .get("issn")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(|i| i.get("number"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            (
                ser.get("title").and_then(Value::as_str).map(ToOwned::to_owned),
                ser.get("short_title").and_then(Value::as_str).map(ToOwned::to_owned),
                ser.get("volume").and_then(Value::as_str).map(ToOwned::to_owned),
                ser.get("issue").and_then(Value::as_str).map(ToOwned::to_owned),
                ser.get("publisher").and_then(Value::as_str).map(ToOwned::to_owned),
                issn_val,
            )
        } else {
            (None, None, None, None, None, None)
        };

    let booktitle = book_first
        .and_then(|b| b.get("title"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let isbn = book_first
        .and_then(|b| b.get("isbn"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|x| x.get("number").or(Some(x)))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let publisher = publisher.or_else(|| {
        book_first
            .and_then(|b| b.get("publisher"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    });

    let year = item
        .get("year")
        .and_then(|y| y.as_i64().or_else(|| y.as_str()?.parse::<i64>().ok()));

    let mut msc_codes: Vec<String> = Vec::new();
    if let Some(mscs) = item.get("msc").and_then(Value::as_array) {
        for m in mscs {
            if let Some(code) = m.get("code").and_then(Value::as_str) {
                msc_codes.push(code.to_string());
            }
        }
    }
    let msc_primary = msc_codes.first().cloned();
    let msc_secondary = if msc_codes.len() > 1 {
        msc_codes[1..].to_vec()
    } else {
        Vec::new()
    };

    let mut doi = None;
    let mut arxiv_id = None;
    if let Some(links) = item.get("links").and_then(Value::as_array) {
        for l in links {
            let ltype = l.get("type").and_then(Value::as_str).unwrap_or("");
            let ident = l.get("identifier").and_then(Value::as_str).unwrap_or("");
            if ltype == "doi" && doi.is_none() {
                doi = normalize_doi(ident);
            } else if ltype == "arxiv" && arxiv_id.is_none() && !ident.is_empty() {
                arxiv_id = Some(ident.to_string());
            }
        }
    }

    let zbl_number = item
        .get("identifier")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let url = item
        .get("zbmath_url")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let abstract_text = item
        .get("editorial_contributions")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(|e| e.get("text"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let license_raw = item
        .get("license")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("{s} (zbMATH Open / FIZ Karlsruhe)"))
        .unwrap_or_else(|| ZBMATH_LICENSE_NOTE.to_string());

    Some(BibliographicRecord {
        work_type,
        title,
        authors,
        journal_abbrev,
        journal_full,
        booktitle,
        series: None,
        volume,
        issue_number,
        year,
        pages,
        edition: None,
        note: None,
        publisher,
        address: None,
        isbn,
        issn,
        coden: None,
        doi,
        arxiv_id,
        arxiv_version: None,
        mr_number: None,
        mr_number_display: None,
        mr_reviewer: None,
        zbl_number,
        url,
        msc_primary,
        msc_secondary,
        abstract_text,
        raw_mrlookup_bibtex: None,
        source: "zbmath".to_string(),
        license_note: Some(license_raw),
    })
}

fn parse_comma_author(raw: &str, role: &str) -> BibliographicAuthor {
    if let Some((fam, giv)) = raw.split_once(',') {
        BibliographicAuthor {
            family_name: fam.trim().to_string(),
            given_name: giv.trim().to_string(),
            role: role.to_string(),
        }
    } else {
        let parts: Vec<&str> = raw.split_whitespace().collect();
        if parts.len() >= 2 {
            BibliographicAuthor {
                family_name: parts.last().unwrap_or(&"").to_string(),
                given_name: parts[..parts.len() - 1].join(" "),
                role: role.to_string(),
            }
        } else {
            BibliographicAuthor {
                family_name: raw.trim().to_string(),
                given_name: String::new(),
                role: role.to_string(),
            }
        }
    }
}

fn normalize_issn_cmp(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase()
}

fn issns_match(item: &Value, query_issn: &str) -> bool {
    let target = normalize_issn_cmp(query_issn);
    if target.len() < 7 {
        return false;
    }
    let Some(list) = item.get("issn").and_then(Value::as_array) else {
        return false;
    };
    list.iter().any(|entry| {
        entry
            .get("number")
            .or(Some(entry))
            .and_then(Value::as_str)
            .map(normalize_issn_cmp)
            .is_some_and(|n| n == target)
    })
}

fn parse_zbmath_serial_abbrev(
    body: &str,
    query_name: Option<&str>,
    query_issn: Option<&str>,
) -> Result<Option<String>> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Ok(None);
    };
    let Some(arr) = v.get("result").and_then(Value::as_array) else {
        return Ok(None);
    };
    for item in arr {
        let Some(short_title) = item
            .get("short_title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if !looks_abbreviated_journal(short_title) {
            continue;
        }
        let item_title = item
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let matched_by_issn = query_issn.is_some_and(|sn| issns_match(item, sn));
        let matched_by_title = query_name
            .is_some_and(|qn| !item_title.is_empty() && serial_titles_match(item_title, qn));
        if matched_by_issn || matched_by_title {
            if let Some(qn) = query_name {
                if normalize_math_text(short_title) == normalize_math_text(qn) {
                    continue;
                }
            }
            return Ok(Some(short_title.to_string()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn serial_lookup_resolves_short_title_by_issn_or_title_from_cache() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("zbm_serial.db"), 2).unwrap();
        let client = ZbMathClient::new("PaperoTest/0.2".into()).unwrap();

        let body = r#"{
            "result": [
                {
                    "id": 465,
                    "title": "Inventiones Mathematicae",
                    "short_title": "Invent. Math.",
                    "acronym": null,
                    "issn": [
                        {"number": "0020-9910", "type": "print"},
                        {"number": "1432-1297", "type": "electronic"}
                    ]
                }
            ]
        }"#;
        store_http_cache(&db, "zbmath_serial", "sn:0020-9910", 200, body).unwrap();

        let by_issn = client
            .lookup_serial_abbrev(&db, Some("Inventiones Mathematicae"), Some("0020-9910"))
            .await
            .unwrap();
        assert_eq!(by_issn.as_deref(), Some("Invent. Math."));

        let esoteric = r#"{
            "result": [
                {
                    "id": 99999,
                    "title": "Carpathian Journal of Weird Examples",
                    "short_title": "Carpathian J. Weird Ex.",
                    "issn": []
                }
            ]
        }"#;
        let ck = format!("ti:{}", normalize_math_text("Carpathian Journal of Weird Examples"));
        store_http_cache(&db, "zbmath_serial", &ck, 200, esoteric).unwrap();

        let by_title = client
            .lookup_serial_abbrev(&db, Some("Carpathian Journal of Weird Examples"), None)
            .await
            .unwrap();
        assert_eq!(by_title.as_deref(), Some("Carpathian J. Weird Ex."));
    }
}
