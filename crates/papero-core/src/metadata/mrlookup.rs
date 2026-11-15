use crate::bibtex::parse_bibtex_entries;
use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::metadata::extract::{extract_prose_core_title, normalize_isbn};
use crate::metadata::limiter::{load_http_cache, store_http_cache, SequentialRateLimiter};
use crate::metadata::models::{
    clean_author_surname, clean_metadata_text, normalize_arxiv_base_id, normalize_doi,
    normalize_mr_digits, BibliographicAuthor, BibliographicRecord, CandidateMatch,
    ExtractedIdentifiers,
};
use crate::text_norm::normalize_math_text;
use regex::Regex;
use reqwest::Client;
use std::collections::HashSet;
use std::time::Duration;
use strsim::jaro_winkler;

const MRLOOKUP_URL: &str = "https://mathscinet.ams.org/mrlookup";

/// Client for MathSciNet's free MRLookup web form (no subscription required).
/// NOTE: AMS has no official public REST API for MRLookup; toggleable in Settings and
/// strictly rate-limited (`<= 1 req / 5s`, concurrency = 1) to avoid bulk scraping.
#[derive(Debug)]
pub struct MrLookupClient {
    http: Client,
    limiter: SequentialRateLimiter,
    endpoint: String,
}

impl MrLookupClient {
    pub fn new(user_agent: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|e| CoreError::Network(format!("mrlookup client init: {e}")))?;
        Ok(Self {
            http,
            limiter: SequentialRateLimiter::new(Duration::from_secs(5)),
            endpoint: MRLOOKUP_URL.to_string(),
        })
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>, interval: Duration) -> Self {
        self.endpoint = endpoint.into();
        self.limiter = SequentialRateLimiter::new(interval);
        self
    }

    pub async fn lookup(
        &self,
        db: &Database,
        ids: &ExtractedIdentifiers,
    ) -> Result<Vec<CandidateMatch>> {
        let mut form = Vec::<(&str, String)>::new();
        let cache_key;

        let clean_au = ids
            .heuristic_first_author
            .as_deref()
            .and_then(clean_author_surname)
            .unwrap_or_default();
        let kw_fallback = ids.filename_keywords.join(" ");
        let mut can_retry_title_only: Option<(String, String)> = None;
        let mut prose_core_retry: Option<(String, String)> = None;

        if let Some(mr) = ids.mr_number.as_deref() {
            if let Some(ti) = ids.heuristic_title.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                form.push(("ti", clean_mrlookup_title_query(ti)));
            } else if !kw_fallback.is_empty() {
                form.push(("ti", kw_fallback.clone()));
            }
            form.push(("au", clean_au.clone()));
            if let Some(yr) = ids.heuristic_year {
                form.push(("year", yr.to_string()));
            }
            cache_key = format!(
                "mr:{mr}:ti:{}",
                normalize_math_text(ids.heuristic_title.as_deref().unwrap_or(&kw_fallback))
            );
        } else {
            let ti_raw = ids.heuristic_title.as_deref().unwrap_or("").trim();
            let ti = if ti_raw.is_empty() {
                kw_fallback.as_str()
            } else {
                ti_raw
            };
            if ti.is_empty() && clean_au.is_empty() {
                return Ok(Vec::new());
            }
            form.push(("au", clean_au.clone()));
            let ti_words = normalize_math_text(ti).split_whitespace().count();
            let cleaned_ti_q = clean_mrlookup_title_query(ti);
            if !cleaned_ti_q.is_empty() {
                form.push(("ti", cleaned_ti_q.clone()));
            }
            // Only send `year` to MRLookup's strict AND filter when title is absent/short (< 3 words),
            // so preprints whose PDF year precedes journal publication still match and get scored locally.
            let form_year = if ti_words < 3 { ids.heuristic_year } else { None };
            if let Some(yr) = form_year {
                form.push(("year", yr.to_string()));
            }
            cache_key = format!(
                "q:ti={}:au={}:yr={}",
                normalize_math_text(ti),
                normalize_math_text(&clean_au),
                form_year.unwrap_or(0)
            );
            if !clean_au.is_empty() && ti_words >= 4 && !cleaned_ti_q.is_empty() {
                let fallback_ck = format!("q:ti={}:au=:yr=0", normalize_math_text(ti));
                can_retry_title_only = Some((cleaned_ti_q.clone(), fallback_ck));
            }
            if let Some(prose_ti) = extract_prose_core_title(ti) {
                let clean_prose = clean_mrlookup_title_query(&prose_ti);
                if !clean_prose.is_empty() && clean_prose != cleaned_ti_q {
                    let p_ck = format!(
                        "qp:ti={}:au={}",
                        normalize_math_text(&prose_ti),
                        normalize_math_text(&clean_au)
                    );
                    prose_core_retry = Some((clean_prose, p_ck));
                }
            }
        }

        if form.is_empty() {
            return Ok(Vec::new());
        }

        let body = self.fetch_mrlookup_body(db, &mut form, &cache_key).await?;
        let mut hits = parse_mrlookup_html_to_candidates(&body, ids);

        // If a noisy heuristic author on page 1 caused MRLookup's strict AND `(ti + au)` filter
        // to return 0 good hits on a >= 4-word title, retry with `ti` alone.
        let mut top_score = hits.first().map(|c| c.score).unwrap_or(0.0);
        if top_score < 0.55 {
            if let Some((ti_only_q, fallback_ck)) = can_retry_title_only {
                let mut fb_form = vec![("au", String::new()), ("ti", ti_only_q)];
                if let Ok(fb_body) = self.fetch_mrlookup_body(db, &mut fb_form, &fallback_ck).await {
                    let mut ids_no_au = ids.clone();
                    ids_no_au.heuristic_first_author = None;
                    ids_no_au.heuristic_authors.clear();
                    let fb_hits = parse_mrlookup_html_to_candidates(&fb_body, &ids_no_au);
                    if fb_hits.first().map(|c| c.score).unwrap_or(0.0) > top_score {
                        hits = fb_hits;
                        top_score = hits.first().map(|c| c.score).unwrap_or(0.0);
                    }
                }
            }
        }

        // If PDF math symbols/formulas in `ti` blocked MRLookup's strict word AND filter, retry with clean prose core
        if top_score < 0.55 {
            if let Some((prose_q, p_ck)) = prose_core_retry {
                let mut p_form = vec![("au", clean_au), ("ti", prose_q)];
                if let Ok(p_body) = self.fetch_mrlookup_body(db, &mut p_form, &p_ck).await {
                    let p_hits = parse_mrlookup_html_to_candidates(&p_body, ids);
                    if p_hits.first().map(|c| c.score).unwrap_or(0.0) > top_score {
                        hits = p_hits;
                    }
                }
            }
        }

        Ok(hits)
    }

    async fn fetch_mrlookup_body(
        &self,
        db: &Database,
        form: &mut Vec<(&str, String)>,
        cache_key: &str,
    ) -> Result<String> {
        form.push(("jrnl", String::new()));
        form.push(("ipage", String::new()));
        form.push(("fpage", String::new()));
        form.push(("format", "bibtex".to_string()));
        form.push(("Search", "Search".to_string()));

        if let Some(cached) = load_http_cache(db, "mrlookup", cache_key)? {
            if cached.status_code >= 400 {
                return Ok(String::new());
            }
            return Ok(cached.body);
        }

        self.limiter.acquire().await;
        let encoded = encode_mrlookup_form(form);
        let resp = self
            .http
            .post(&self.endpoint)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(encoded)
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("mrlookup request: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| CoreError::Network(format!("mrlookup read body: {e}")))?;
        store_http_cache(db, "mrlookup", cache_key, status, &text)?;
        if status >= 400 {
            return Err(CoreError::Network(format!("mrlookup HTTP {status}")));
        }
        Ok(text)
    }
}

fn clean_mrlookup_title_query(raw: &str) -> String {
    let cleaned = clean_metadata_text(raw);
    let mut out = String::with_capacity(cleaned.len());
    let mut prev_space = true;
    for ch in cleaned.chars() {
        match ch {
            '{' | '}' | '$' | '\\' | '"' | '“' | '”' => {}
            _ if ch.is_alphanumeric() || ch == '\'' => {
                out.push(ch);
                prev_space = false;
            }
            _ => {
                if !prev_space {
                    out.push(' ');
                    prev_space = true;
                }
            }
        }
    }
    out.trim().to_string()
}

fn encode_mrlookup_form(fields: &[(&str, String)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in fields {
        serializer.append_pair(k, v);
    }
    serializer.finish()
}

/// Extracts verbatim `<pre>` BibTeX blocks from MRLookup HTML and scores each entry.
pub fn parse_mrlookup_html_to_candidates(
    html_body: &str,
    ids: &ExtractedIdentifiers,
) -> Vec<CandidateMatch> {
    let pre_re = Regex::new(r"(?is)<pre[^>]*>(.*?)</pre>").unwrap();
    let tag_re = Regex::new(r"<[^>]+>").unwrap();
    let mut matches = Vec::new();

    for cap in pre_re.captures_iter(html_body) {
        let inner = tag_re.replace_all(&cap[1], "");
        let unescaped = decode_basic_html_entities(&inner);
        for parsed in parse_bibtex_entries(&unescaped) {
            let mut rec = parsed.record;
            rec.source = "mrlookup".to_string();
            let score = score_candidate(&rec, ids);
            matches.push(CandidateMatch {
                score,
                source: "mrlookup".to_string(),
                record: rec,
            });
        }
    }

    matches.sort_by(|a, b| b.score.total_cmp(&a.score));
    matches
}

pub fn decode_basic_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

pub fn score_candidate(rec: &BibliographicRecord, ids: &ExtractedIdentifiers) -> f32 {
    // 1. Exact identifier matches (normalized across providers)
    if let (Some(q_mr), Some(r_mr)) = (
        ids.mr_number.as_deref().and_then(normalize_mr_digits),
        rec.mr_number.as_deref().and_then(normalize_mr_digits),
    ) {
        if q_mr == r_mr {
            return 0.99;
        }
    }
    if let (Some(q_zbl), Some(r_zbl)) = (ids.zbl_number.as_deref(), rec.zbl_number.as_deref()) {
        if !q_zbl.trim().is_empty() && q_zbl.trim().eq_ignore_ascii_case(r_zbl.trim()) {
            return 0.99;
        }
    }
    if let (Some(q_doi), Some(r_doi)) = (
        ids.doi.as_deref().and_then(normalize_doi),
        rec.doi.as_deref().and_then(normalize_doi),
    ) {
        if q_doi == r_doi {
            return 0.99;
        }
    }
    if let (Some(q_ax), Some(r_ax)) = (
        ids.arxiv_id.as_deref().and_then(normalize_arxiv_base_id),
        rec.arxiv_id.as_deref().and_then(normalize_arxiv_base_id),
    ) {
        if q_ax == r_ax {
            return 0.99;
        }
    }
    if let (Some(q_isbn), Some(r_isbn)) = (
        ids.isbn.as_deref().and_then(normalize_isbn),
        rec.isbn.as_deref().and_then(normalize_isbn),
    ) {
        if q_isbn == r_isbn {
            return 0.99;
        }
    }

    let (has_query_author, author_sim) = compute_author_similarity(ids, &rec.authors);
    let year_diff = match (ids.heuristic_year, rec.year) {
        (Some(qy), Some(ry)) => Some((qy - ry).abs()),
        _ => None,
    };
    let year_bonus = match year_diff {
        Some(0) => 0.07,
        Some(1) => 0.05,
        Some(2) => 0.02,
        _ => 0.0,
    };

    // 2. Title + Author + Year scoring when a heuristic/layout/embedded title is present
    if let Some(q_title) = ids
        .heuristic_title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let (title_sim, q_sig_words, q_norm_len) = compute_title_similarity(q_title, &rec.title);

        let mut score = if has_query_author {
            title_sim * 0.72 + author_sim * 0.21 + year_bonus
        } else {
            title_sim * 0.82 + year_bonus
        };

        // Recalibrated boost for unmistakable exact/near-exact multi-word title matches
        let strong_distinctive_title =
            (q_sig_words >= 4 && q_norm_len >= 20 && title_sim >= 0.94)
                || (q_sig_words == 3
                    && q_norm_len >= 16
                    && title_sim >= 0.98
                    && (!has_query_author || author_sim >= 0.85 || year_bonus >= 0.05));

        if strong_distinctive_title {
            let floor = if author_sim >= 0.85 {
                0.93 + year_bonus.min(0.05)
            } else if !has_query_author {
                if q_sig_words >= 5 && q_norm_len >= 26 && title_sim >= 0.98 {
                    0.95 + year_bonus.min(0.03)
                } else if q_sig_words >= 4 && title_sim >= 0.96 {
                    0.92 + year_bonus.min(0.05)
                } else {
                    0.90 + year_bonus.min(0.05)
                }
            } else if year_bonus >= 0.05 {
                0.89
            } else if q_sig_words >= 5 && q_norm_len >= 28 && title_sim >= 0.97 {
                0.88
            } else {
                0.0
            };
            score = score.max(floor);
        } else if q_sig_words >= 4 && title_sim >= 0.91 && author_sim >= 0.90 {
            score = score.max(0.87 + year_bonus.min(0.06));
        }

        if is_erratum_or_review_mismatch(q_title, &rec.title)
            || has_series_part_mismatch(q_title, &rec.title)
        {
            score *= 0.45;
        }

        return score.clamp(0.0, 0.98);
    }

    // 3. Fallback scoring for author (+ optional year / filename keywords) when title is absent
    if has_query_author && author_sim >= 0.88 {
        let mut score = match year_diff {
            Some(0) => 0.62 * author_sim,
            Some(1) => 0.54 * author_sim,
            None => 0.48 * author_sim,
            _ => 0.20 * author_sim,
        };
        if !ids.filename_keywords.is_empty() {
            let r_norm = normalize_math_text(&clean_metadata_text(&rec.title));
            let matched = ids
                .filename_keywords
                .iter()
                .filter(|kw| {
                    let kw_norm = normalize_math_text(kw);
                    !kw_norm.is_empty() && r_norm.contains(&kw_norm)
                })
                .count();
            let ratio = (matched as f32) / (ids.filename_keywords.len() as f32);
            score += ratio * 0.26;
        }
        return score.clamp(0.0, 0.94);
    }

    0.0
}

pub(crate) fn significant_title_tokens(norm: &str) -> HashSet<&str> {
    norm.split_whitespace()
        .filter(|w| {
            w.len() >= 3
                && !matches!(
                    *w,
                    "the"
                        | "and"
                        | "for"
                        | "with"
                        | "from"
                        | "into"
                        | "over"
                        | "under"
                        | "via"
                        | "sur"
                        | "les"
                        | "des"
                        | "une"
                        | "dans"
                        | "und"
                        | "der"
                        | "die"
                        | "das"
                        | "uber"
                        | "del"
                        | "dei"
                        | "con"
                        | "una"
                        | "sul"
                        | "sulla"
                )
        })
        .collect()
}

pub(crate) fn pair_title_sim(q_norm: &str, r_norm: &str) -> f32 {
    if q_norm.is_empty() || r_norm.is_empty() {
        return 0.0;
    }
    if q_norm == r_norm {
        return 1.0;
    }
    let jw = jaro_winkler(q_norm, r_norm) as f32;
    let q_toks = significant_title_tokens(q_norm);
    let r_toks = significant_title_tokens(r_norm);
    if q_toks.is_empty() || r_toks.is_empty() {
        return jw;
    }
    let overlap = q_toks.intersection(&r_toks).count() as f32;
    let prec = overlap / (r_toks.len() as f32);
    let rec = overlap / (q_toks.len() as f32);
    let f1 = if prec + rec > 0.0 {
        2.0 * prec * rec / (prec + rec)
    } else {
        0.0
    };
    let mut sim = (0.50 * jw + 0.50 * f1).clamp(0.0, 1.0);

    // Substring / full query-token containment boost ONLY when query covers a substantial share
    // of the candidate title (`prec >= 0.52`), preventing 3-word fragments from matching long unrelated titles.
    if rec >= 0.99 {
        if q_toks.len() >= 3 && prec >= 0.52 {
            let cont_floor = if r_norm.contains(q_norm) && q_norm.len() >= 18 {
                (0.84 + 0.12 * prec).min(0.95)
            } else {
                (0.76 + 0.18 * prec).min(0.93)
            };
            sim = sim.max(cont_floor);
        } else if q_toks.len() >= 3 {
            sim = sim.min(0.56 + 0.35 * prec);
        }
    }
    sim
}

pub(crate) fn is_erratum_or_review_mismatch(q_raw: &str, r_raw: &str) -> bool {
    let q_low = normalize_title_words(q_raw);
    let r_low = normalize_title_words(r_raw);
    for marker in [
        "erratum",
        "errata",
        "corrigendum",
        "corrigenda",
        "correction to ",
        "author correction",
        "publisher correction",
        "retraction",
        "retracted",
        "addendum to ",
        "book review",
    ] {
        if r_low.contains(marker) && !q_low.contains(marker.trim()) {
            return true;
        }
    }
    false
}

fn parse_part_numeral(tok: &str) -> Option<u32> {
    match tok {
        "i" | "1" => Some(1),
        "ii" | "2" => Some(2),
        "iii" | "3" => Some(3),
        "iv" | "4" => Some(4),
        "v" | "5" => Some(5),
        "vi" | "6" => Some(6),
        "vii" | "7" => Some(7),
        "viii" | "8" => Some(8),
        "ix" | "9" => Some(9),
        "x" | "10" => Some(10),
        "xi" | "11" => Some(11),
        "xii" | "12" => Some(12),
        _ => None,
    }
}

fn has_series_part_mismatch(q_raw: &str, r_raw: &str) -> bool {
    let q_norm = normalize_title_words(&clean_metadata_text(q_raw));
    let r_norm = normalize_title_words(&clean_metadata_text(r_raw));
    let extract_part = |norm: &str| -> Option<u32> {
        let toks: Vec<&str> = norm.split_whitespace().collect();
        for win in toks.windows(2) {
            if matches!(win[0], "part" | "teil" | "vol" | "volume" | "no" | "nr") {
                if let Some(num) = parse_part_numeral(win[1]) {
                    return Some(num);
                }
            }
        }
        toks.last().copied().and_then(parse_part_numeral)
    };
    matches!(
        (extract_part(&q_norm), extract_part(&r_norm)),
        (Some(qp), Some(rp)) if qp != rp
    )
}

pub(crate) fn titles_represent_same_work(a: &str, b: &str) -> bool {
    if is_erratum_or_review_mismatch(a, b) || has_series_part_mismatch(a, b) {
        return false;
    }
    let (sim, sig, _) = compute_title_similarity(a, b);
    sim >= 0.93 && sig >= 3
}

pub(crate) fn significant_title_words_vec(raw: &str) -> Vec<String> {
    let clean = clean_metadata_text(raw);
    let main = split_main_title(&clean).unwrap_or(&clean);
    let norm = normalize_title_words(main);
    significant_title_tokens(&norm)
        .into_iter()
        .map(ToOwned::to_owned)
        .collect()
}

fn split_main_title(raw: &str) -> Option<&str> {
    for sep in [":", " — ", " – ", " - "] {
        if let Some((head, _)) = raw.split_once(sep) {
            let trimmed = head.trim();
            if trimmed.split_whitespace().count() >= 4 && trimmed.len() >= 22 {
                return Some(trimmed);
            }
        }
    }
    None
}

pub(crate) fn normalize_title_words(s: &str) -> String {
    let base = normalize_math_text(s);
    let mut out = String::with_capacity(base.len());
    let mut prev_space = true;
    for ch in base.chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

pub(crate) fn compute_title_similarity(q_raw: &str, r_raw: &str) -> (f32, usize, usize) {
    let q_clean = clean_metadata_text(q_raw);
    let r_clean = clean_metadata_text(r_raw);
    let q_norm = normalize_title_words(&q_clean);
    let r_norm = normalize_title_words(&r_clean);
    let sig_count = significant_title_tokens(&q_norm).len();
    let mut best = pair_title_sim(&q_norm, &r_norm);

    // Subtitle-tolerant comparison when one side omits ": Subtitle"
    let q_main = split_main_title(&q_clean).map(normalize_title_words);
    let r_main = split_main_title(&r_clean).map(normalize_title_words);
    if q_main.is_some() || r_main.is_some() {
        let qm = q_main.as_deref().unwrap_or(&q_norm);
        let rm = r_main.as_deref().unwrap_or(&r_norm);
        if qm.split_whitespace().count() >= 4
            && rm.split_whitespace().count() >= 4
            && qm.len() >= 20
            && rm.len() >= 20
        {
            best = best.max(pair_title_sim(qm, rm) * 0.97);
        }
    }

    (best, sig_count, q_norm.len())
}

fn compute_author_similarity(
    ids: &ExtractedIdentifiers,
    rec_authors: &[BibliographicAuthor],
) -> (bool, f32) {
    let mut query_authors: Vec<&str> = Vec::new();
    if let Some(first) = ids
        .heuristic_first_author
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        query_authors.push(first);
    }
    for a in &ids.heuristic_authors {
        let t = a.trim();
        if !t.is_empty() && !query_authors.contains(&t) {
            query_authors.push(t);
        }
    }

    if query_authors.is_empty() {
        return (false, 0.0);
    }
    if rec_authors.is_empty() {
        return (true, 0.0);
    }

    let mut best = 0.0_f32;
    let mut matched_coauthors = 0usize;

    for qa in &query_authors {
        let q_norm = normalize_math_text(qa);
        let q_surname = clean_author_surname(qa)
            .map(|s| normalize_math_text(&s))
            .unwrap_or_default();

        let qa_best = rec_authors
            .iter()
            .map(|ra| {
                let fam = normalize_math_text(&ra.family_name);
                let full = normalize_math_text(&format!("{} {}", ra.given_name, ra.family_name));
                let mut s = jaro_winkler(&q_norm, &full) as f32;
                if !q_surname.is_empty() && !fam.is_empty() {
                    if q_surname == fam {
                        s = s.max(0.98);
                    } else {
                        let fam_jw = jaro_winkler(&q_surname, &fam) as f32;
                        if fam_jw >= 0.92 {
                            s = s.max(0.92);
                        }
                    }
                }
                if fam.len() >= 3 && q_norm.split_whitespace().any(|w| w == fam) {
                    s = s.max(0.96);
                }
                s
            })
            .fold(0.0_f32, f32::max);

        if qa_best >= 0.90 {
            matched_coauthors += 1;
        }
        best = best.max(qa_best);
    }

    if matched_coauthors >= 2 {
        best = (best + 0.02).min(1.0);
    }
    (true, best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::models::WorkType;

    #[test]
    fn scores_exact_title_high_even_without_author_or_with_preprint_year_shift() {
        let rec = BibliographicRecord {
            work_type: WorkType::Article,
            title: "On the existence, uniqueness and approximation of saddle-point problems arising from Lagrangian multipliers".into(),
            authors: vec![BibliographicAuthor {
                family_name: "Brezzi".into(),
                given_name: "Franco".into(),
                role: "author".into(),
            }],
            year: Some(1974),
            arxiv_id: Some("arXiv:0704.0001v2".into()),
            ..Default::default()
        };

        // 1. Normalized arXiv match despite "arXiv:" prefix and version suffix
        let ids_arxiv = ExtractedIdentifiers {
            arxiv_id: Some("0704.0001".into()),
            ..Default::default()
        };
        assert!((score_candidate(&rec, &ids_arxiv) - 0.99).abs() < 1e-4);

        // 2. Exact title when author was missed on page 1 and year shifted by 1 (preprint 1973 -> print 1974)
        let ids_title_only = ExtractedIdentifiers {
            heuristic_title: Some("On the Existence, Uniqueness and Approximation of Saddle-Point Problems Arising from Lagrangian Multipliers".into()),
            heuristic_year: Some(1973),
            ..Default::default()
        };
        assert!(score_candidate(&rec, &ids_title_only) >= 0.90);

        // 3. Unrelated paper sharing "On the existence and uniqueness of..." prefix scores low
        let rec_unrelated = BibliographicRecord {
            work_type: WorkType::Article,
            title: "On the existence and uniqueness of global weak solutions to Navier-Stokes equations".into(),
            authors: vec![BibliographicAuthor {
                family_name: "Leray".into(),
                given_name: "Jean".into(),
                role: "author".into(),
            }],
            year: Some(1934),
            ..Default::default()
        };
        assert!(score_candidate(&rec_unrelated, &ids_title_only) < 0.55);

        // 4. Scan fallback: author + year + filename keyword ("Brezzi_1974_Saddle.pdf")
        let ids_scan = ExtractedIdentifiers {
            heuristic_first_author: Some("Brezzi".into()),
            heuristic_year: Some(1974),
            filename_keywords: vec!["saddle".into(), "lagrangian".into()],
            ..Default::default()
        };
        assert!(score_candidate(&rec, &ids_scan) >= 0.85);
    }
}
