use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::metadata::limiter::{load_http_cache, store_http_cache, SequentialRateLimiter};
use crate::metadata::models::{
    clean_author_surname, normalize_doi, BibliographicAuthor, BibliographicRecord, CandidateMatch,
    ExtractedIdentifiers, WorkType,
};
use crate::metadata::mrlookup::score_candidate;
use crate::text_norm::normalize_math_text;
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::Client;
use std::time::Duration;

const ARXIV_API: &str = "https://export.arxiv.org/api/query";

/// Client for arXiv Atom export API (`1 req / 3s`, concurrency = 1, cached).
#[derive(Debug)]
pub struct ArxivClient {
    http: Client,
    limiter: SequentialRateLimiter,
}

impl ArxivClient {
    pub fn new(user_agent: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| CoreError::Network(format!("arxiv client init: {e}")))?;
        Ok(Self {
            http,
            limiter: SequentialRateLimiter::new(Duration::from_secs(3)),
        })
    }

    pub async fn lookup(
        &self,
        db: &Database,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        let (params, cache_key): (Vec<(&str, String)>, String) =
            if let Some(ax) = ids.arxiv_id.as_deref() {
                (vec![("id_list", ax.to_string())], format!("arxiv:{ax}"))
            } else if let Some(ti) = ids.heuristic_title.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                let clean_ti = ti.replace('"', " ");
                let q = if let Some(last) = ids.heuristic_first_author.as_deref().and_then(clean_author_surname) {
                    format!("ti:\"{clean_ti}\" AND au:{last}")
                } else {
                    format!("ti:\"{clean_ti}\"")
                };
                let ck = format!("q:{}", normalize_math_text(&q));
                (vec![("search_query", q), ("max_results", "5".to_string())], ck)
            } else {
                return Ok(Vec::new());
            };

        let body = if let Some(c) = load_http_cache(db, "arxiv", &cache_key)? {
            if c.status_code >= 400 {
                return Ok(Vec::new());
            }
            c.body
        } else {
            self.limiter.acquire().await;
            let req_url = url::Url::parse_with_params(ARXIV_API, &params)
                .map_err(|e| CoreError::Network(format!("arxiv url build: {e}")))?;
            let resp = self
                .http
                .get(req_url)
                .send()
                .await
                .map_err(|e| CoreError::Network(format!("arxiv request: {e}")))?;
            let status = resp.status().as_u16();
            let text = resp
                .text()
                .await
                .map_err(|e| CoreError::Network(format!("arxiv read body: {e}")))?;
            store_http_cache(db, "arxiv", &cache_key, status, &text)?;
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        Ok(parse_arxiv_atom(&body, ids))
    }
}

pub fn parse_arxiv_atom(xml: &str, ids: &ExtractedIdentifiers) -> Vec<CandidateMatch> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut out = Vec::new();
    let mut in_entry = false;
    let mut in_author = false;
    let mut cur_tag = String::new();

    let mut cur_title = String::new();
    let mut cur_summary = String::new();
    let mut cur_id = String::new();
    let mut cur_published = String::new();
    let mut cur_doi = String::new();
    let mut cur_journal_ref = String::new();
    let mut cur_author_name = String::new();
    let mut cur_authors: Vec<BibliographicAuthor> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name().as_ref().to_string();
                match name.as_str() {
                    "entry" => {
                        in_entry = true;
                        cur_title.clear();
                        cur_summary.clear();
                        cur_id.clear();
                        cur_published.clear();
                        cur_doi.clear();
                        cur_journal_ref.clear();
                        cur_authors.clear();
                    }
                    "author" if in_entry => {
                        in_author = true;
                        cur_author_name.clear();
                    }
                    _ => cur_tag = name,
                }
            }
            Ok(Event::End(e)) => {
                let name = e.name().as_ref().to_string();
                if name == "author" && in_author {
                    let trimmed = cur_author_name.trim();
                    if !trimmed.is_empty() {
                        let parts: Vec<&str> = trimmed.split_whitespace().collect();
                        let (giv, fam) = if parts.len() >= 2 {
                            (
                                parts[..parts.len() - 1].join(" "),
                                parts.last().unwrap_or(&"").to_string(),
                            )
                        } else {
                            (String::new(), trimmed.to_string())
                        };
                        cur_authors.push(BibliographicAuthor {
                            family_name: fam,
                            given_name: giv,
                            role: "author".to_string(),
                        });
                    }
                    in_author = false;
                } else if name == "entry" && in_entry {
                    in_entry = false;
                    let clean_title = cur_title.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !clean_title.is_empty() {
                        let (ax_id, ax_ver) = parse_arxiv_url_id(&cur_id);
                        let year = cur_published
                            .get(..4)
                            .and_then(|s| s.parse::<i64>().ok());
                        let doi = normalize_doi(&cur_doi);
                        let url = if !cur_id.trim().is_empty() {
                            Some(cur_id.trim().to_string())
                        } else {
                            None
                        };
                        let rec = BibliographicRecord {
                            work_type: WorkType::Article,
                            title: clean_title,
                            authors: cur_authors.clone(),
                            journal_abbrev: if cur_journal_ref.trim().is_empty() {
                                None
                            } else {
                                Some(cur_journal_ref.trim().to_string())
                            },
                            year,
                            doi,
                            arxiv_id: ax_id,
                            arxiv_version: ax_ver,
                            url,
                            abstract_text: if cur_summary.trim().is_empty() {
                                None
                            } else {
                                Some(cur_summary.split_whitespace().collect::<Vec<_>>().join(" "))
                            },
                            source: "arxiv".to_string(),
                            ..Default::default()
                        };
                        let score = score_candidate(&rec, ids);
                        out.push(CandidateMatch {
                            score,
                            source: "arxiv".to_string(),
                            record: rec,
                        });
                    }
                }
                cur_tag.clear();
            }
            Ok(Event::Text(t)) if in_entry => {
                let txt = t.as_ref().to_string();
                if in_author && cur_tag == "name" {
                    cur_author_name.push_str(&txt);
                } else {
                    match cur_tag.as_str() {
                        "title" => cur_title.push_str(&txt),
                        "summary" => cur_summary.push_str(&txt),
                        "id" => cur_id.push_str(&txt),
                        "published" => cur_published.push_str(&txt),
                        "arxiv:doi" | "doi" => cur_doi.push_str(&txt),
                        "arxiv:journal_ref" => cur_journal_ref.push_str(&txt),
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

fn parse_arxiv_url_id(url: &str) -> (Option<String>, Option<String>) {
    let trimmed = url.trim().trim_end_matches('/');
    let raw = trimmed
        .split("/abs/")
        .nth(1)
        .unwrap_or(trimmed);
    if raw.is_empty() {
        return (None, None);
    }
    if let Some((base, ver)) = raw.rsplit_once('v') {
        if !ver.is_empty() && ver.chars().all(|c| c.is_ascii_digit()) {
            return (Some(base.to_string()), Some(format!("v{ver}")));
        }
    }
    (Some(raw.to_string()), None)
}
