use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::metadata::extract::{alternate_isbn_form, normalize_isbn};
use crate::metadata::limiter::{load_http_cache, store_http_cache, SequentialRateLimiter};
use crate::metadata::models::{
    BibliographicAuthor, BibliographicRecord, CandidateMatch, ExtractedIdentifiers, WorkType,
};
use crate::metadata::mrlookup::score_candidate;
use crate::text_norm::normalize_math_text;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;

const HARVARD_ITEMS_URL: &str = "https://api.lib.harvard.edu/v2/items.json";

/// Harvard University LibraryCloud v2 MODS JSON client for academic books & monographs
/// (100% free, no API key required; curated MARC21 catalog with publisher, place, series, edition, pages, ISBN).
#[derive(Debug)]
pub struct HarvardClient {
    http: Client,
    limiter: SequentialRateLimiter,
}

impl HarvardClient {
    pub fn new(user_agent: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| CoreError::Network(format!("harvard client init: {e}")))?;
        Ok(Self {
            http,
            limiter: SequentialRateLimiter::new(Duration::from_millis(250)),
        })
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
            let mut hits = self.query_by_isbn(db, isbn).await?;
            if hits.is_empty() {
                if let Some(alt) = alternate_isbn_form(isbn) {
                    hits = self.query_by_isbn(db, &alt).await?;
                }
            }
            if !hits.is_empty() {
                return Ok(hits);
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

        let body = if let Some(cached) = load_http_cache(db, "harvard", &cache_key)? {
            if cached.status_code >= 400 {
                return Ok(Vec::new());
            }
            cached.body
        } else {
            let mut url = format!(
                "{HARVARD_ITEMS_URL}?title={}&resourceType=text&limit=5",
                url::form_urlencoded::byte_serialize(ti.as_bytes()).collect::<String>()
            );
            if !au.is_empty() {
                url.push_str("&name=");
                url.push_str(
                    &url::form_urlencoded::byte_serialize(au.as_bytes()).collect::<String>(),
                );
            }
            let (status, text) = self.fetch_cached_url(&url).await?;
            if status < 500 && status != 429 {
                store_http_cache(db, "harvard", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        let records = parse_harvard_items_json(&body, None);
        let mut out: Vec<CandidateMatch> = records
            .into_iter()
            .map(|rec| CandidateMatch {
                score: score_candidate(&rec, ids),
                source: "harvard".to_string(),
                record: rec,
            })
            .collect();
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(out)
    }

    async fn query_by_isbn(&self, db: &Database, isbn: &str) -> Result<Vec<CandidateMatch>> {
        let cache_key = format!("isbn:{isbn}");
        let body = if let Some(cached) = load_http_cache(db, "harvard", &cache_key)? {
            if cached.status_code >= 400 {
                return Ok(Vec::new());
            }
            cached.body
        } else {
            let url = format!(
                "{HARVARD_ITEMS_URL}?identifier={}&limit=4",
                url::form_urlencoded::byte_serialize(isbn.as_bytes()).collect::<String>()
            );
            let (status, text) = self.fetch_cached_url(&url).await?;
            if status < 500 && status != 429 {
                store_http_cache(db, "harvard", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        let records = parse_harvard_items_json(&body, Some(isbn));
        Ok(records
            .into_iter()
            .map(|rec| CandidateMatch {
                score: 0.985,
                source: "harvard".to_string(),
                record: rec,
            })
            .collect())
    }

    async fn fetch_cached_url(&self, url: &str) -> Result<(u16, String)> {
        self.limiter.acquire().await;
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("harvard request: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| CoreError::Network(format!("harvard read body: {e}")))?;
        Ok((status, text))
    }
}

fn as_slice_or_single(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(arr) => arr.iter().collect(),
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

fn text_of(v: &Value) -> Option<&str> {
    match v {
        Value::String(s) => Some(s.as_str()),
        Value::Object(map) => map.get("#text").and_then(Value::as_str),
        _ => None,
    }
}

pub(crate) fn clean_marc_field(raw: &str) -> String {
    let mut s = raw
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    loop {
        let next = s
            .trim_end_matches(['/', ':', ';', ','])
            .trim();
        if next.len() == s.len() {
            break;
        }
        s = next;
    }
    s.to_string()
}

pub(crate) fn parse_marc_personal_name(raw: &str, is_editor: bool) -> Option<BibliographicAuthor> {
    static LIFE_DATES_RE: OnceLock<Regex> = OnceLock::new();
    let dates_re = LIFE_DATES_RE.get_or_init(|| {
        Regex::new(r",?\s*(?:ca\.\s*)?(?:fl\.\s*)?(?:1[4-9]\d{2}|20\d{2})\??-(?:1[4-9]\d{2}|20\d{2})?\.?$")
            .unwrap()
    });
    let stripped_role = raw
        .trim()
        .trim_end_matches(", author.")
        .trim_end_matches(", author")
        .trim_end_matches(", editor.")
        .trim_end_matches(", editor")
        .trim_end_matches(", ed.")
        .trim();
    let without_dates = dates_re.replace(stripped_role, "");
    let cleaned = without_dates
        .trim()
        .trim_end_matches(',')
        .trim();
    if cleaned.is_empty() {
        return None;
    }
    let role = if is_editor { "editor" } else { "author" }.to_string();
    if let Some((fam, giv)) = cleaned.split_once(',') {
        let family_name = fam.trim().trim_end_matches('.').trim().to_string();
        let mut given_name = giv.trim().to_string();
        // Strip parenthesized fuller-form expansion like "J.-P. (Jean-Pierre)" -> "Jean-Pierre"
        if let (Some(lp), Some(rp)) = (given_name.find('('), given_name.rfind(')')) {
            if rp > lp + 2 {
                let inside = given_name[lp + 1..rp].trim();
                if inside.chars().all(|c| c.is_alphabetic() || c == '-' || c == ' ' || c == '.') {
                    given_name = inside.to_string();
                }
            }
        }
        // Strip MARC ISBD trailing period on full given names ("Robin." -> "Robin")
        // while preserving genuine initials ("Lawrence C.", "J.-P.").
        if let Some(without_dot) = given_name.strip_suffix('.') {
            let last_tok = without_dot
                .rsplit([' ', '-'])
                .next()
                .unwrap_or(without_dot);
            if last_tok.chars().count() >= 3 {
                given_name = without_dot.trim_end().to_string();
            }
        }
        if family_name.is_empty() {
            return None;
        }
        Some(BibliographicAuthor {
            family_name,
            given_name,
            role,
        })
    } else {
        let parts: Vec<&str> = cleaned.split_whitespace().collect();
        if parts.len() >= 2 {
            Some(BibliographicAuthor {
                family_name: parts.last().unwrap().trim_end_matches('.').to_string(),
                given_name: parts[..parts.len() - 1].join(" "),
                role,
            })
        } else {
            Some(BibliographicAuthor {
                family_name: cleaned.to_string(),
                given_name: String::new(),
                role,
            })
        }
    }
}

pub(crate) fn extract_pages_from_extent(extent: &str) -> Option<String> {
    static PAGES_RE: OnceLock<Regex> = OnceLock::new();
    let re = PAGES_RE.get_or_init(|| {
        Regex::new(r"(?i)\b(\d{2,5})\s*(?:p\.|pages\b|pp\.)").unwrap()
    });
    re.captures(extent).map(|c| c[1].to_string())
}

pub(crate) fn split_series_and_volume(raw_series: &str, explicit_vol: Option<&str>) -> (Option<String>, Option<String>) {
    static SERIES_VOL_RE: OnceLock<Regex> = OnceLock::new();
    let re = SERIES_VOL_RE.get_or_init(|| {
        Regex::new(r"(?i)^(.+?)\s*(?:;|,)\s*(?:v\.|vol\.|volume|no\.|nr\.|n°|bd\.|heft|tome|fasc\.)?\s*([0-9]+[A-Za-z]?)\s*$").unwrap()
    });
    let cleaned = clean_marc_field(raw_series).trim_end_matches('.').trim().to_string();
    if cleaned.is_empty() {
        return (None, explicit_vol.map(clean_marc_field).filter(|s| !s.is_empty()));
    }
    if let Some(ev) = explicit_vol.map(clean_marc_field).filter(|s| !s.is_empty()) {
        let ev_num = ev
            .trim_start_matches(|c: char| !c.is_ascii_digit())
            .trim_end_matches('.')
            .trim()
            .to_string();
        return (
            Some(cleaned),
            Some(if ev_num.is_empty() { ev } else { ev_num }),
        );
    }
    if let Some(cap) = re.captures(&cleaned) {
        let ser = clean_marc_field(&cap[1]).trim_end_matches('.').trim().to_string();
        let vol = cap[2].trim().to_string();
        if !ser.is_empty() {
            return (Some(ser), if vol.is_empty() { None } else { Some(vol) });
        }
    }
    (Some(cleaned), None)
}

pub(crate) fn extract_marc_year(raw: &str) -> Option<i64> {
    static YEAR_RE: OnceLock<Regex> = OnceLock::new();
    let re = YEAR_RE.get_or_init(|| Regex::new(r"\b(1[6-9]\d{2}|20\d{2})\b").unwrap());
    re.captures(raw).and_then(|c| c[1].parse::<i64>().ok())
}

pub fn parse_harvard_items_json(body: &str, fallback_isbn: Option<&str>) -> Vec<BibliographicRecord> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let Some(mods_val) = root.get("items").and_then(|it| it.get("mods")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in as_slice_or_single(mods_val) {
        if let Some(rec) = parse_single_harvard_mods(item, fallback_isbn) {
            out.push(rec);
        }
    }
    out
}

fn parse_single_harvard_mods(m: &Value, fallback_isbn: Option<&str>) -> Option<BibliographicRecord> {
    // 1. Primary titleInfo (skip alternative / uniform / abbreviated titles)
    let title_infos = m.get("titleInfo").map(as_slice_or_single).unwrap_or_default();
    let primary_ti = title_infos
        .iter()
        .copied()
        .find(|ti| ti.get("@type").is_none())
        .or_else(|| title_infos.first().copied())?;

    let non_sort = primary_ti
        .get("nonSort")
        .and_then(text_of)
        .unwrap_or("")
        .trim();
    let main_title = clean_marc_field(primary_ti.get("title").and_then(text_of)?);
    if main_title.is_empty() {
        return None;
    }
    let subtitle = primary_ti
        .get("subTitle")
        .and_then(text_of)
        .map(clean_marc_field)
        .filter(|s| !s.is_empty());

    let mut title = if non_sort.is_empty() {
        main_title
    } else {
        format!("{non_sort} {main_title}")
    };
    if let Some(sub) = subtitle {
        let sub_trimmed = sub.trim_end_matches('.').trim();
        if !sub_trimmed.is_empty()
            && !normalize_math_text(&title).contains(&normalize_math_text(sub_trimmed))
        {
            title = format!("{}: {}", title.trim_end_matches('.'), sub_trimmed);
        }
    }
    title = title.trim_end_matches('.').trim().to_string();

    // 2. Personal authors & editors
    let mut authors = Vec::new();
    if let Some(names) = m.get("name") {
        for n in as_slice_or_single(names) {
            let ntype = n.get("@type").and_then(Value::as_str).unwrap_or("personal");
            if ntype != "personal" {
                continue;
            }
            let mut role_is_editor = false;
            if let Some(roles) = n.get("role") {
                for r in as_slice_or_single(roles) {
                    if let Some(rt) = r.get("roleTerm") {
                        for term in as_slice_or_single(rt) {
                            if let Some(txt) = text_of(term) {
                                let low = txt.to_ascii_lowercase();
                                if low.contains("editor") || low == "ed." || low == "edt" {
                                    role_is_editor = true;
                                }
                            }
                        }
                    }
                }
            }
            let parts = n.get("namePart").map(as_slice_or_single).unwrap_or_default();
            let mut combined_name = String::new();
            for p in parts {
                let ptype = p.get("@type").and_then(Value::as_str).unwrap_or("");
                if ptype == "date" {
                    continue;
                }
                if let Some(txt) = text_of(p) {
                    if txt.to_ascii_lowercase().contains("editor") {
                        role_is_editor = true;
                    }
                    if !combined_name.is_empty() {
                        combined_name.push(' ');
                    }
                    combined_name.push_str(txt.trim());
                }
            }
            if let Some(au) = parse_marc_personal_name(&combined_name, role_is_editor) {
                if !authors.iter().any(|existing: &BibliographicAuthor| {
                    existing.family_name.eq_ignore_ascii_case(&au.family_name)
                        && existing.given_name.eq_ignore_ascii_case(&au.given_name)
                }) {
                    authors.push(au);
                }
            }
        }
    }

    // 3. Publisher, Place (address), Year, Edition
    let mut publisher: Option<String> = None;
    let mut address: Option<String> = None;
    let mut year: Option<i64> = None;
    let mut edition: Option<String> = None;

    if let Some(origins) = m.get("originInfo") {
        for oi in as_slice_or_single(origins) {
            if publisher.is_none() {
                if let Some(pub_val) = oi.get("publisher") {
                    for pv in as_slice_or_single(pub_val) {
                        if let Some(txt) = text_of(pv) {
                            let c = clean_marc_field(txt);
                            if !c.is_empty() {
                                publisher = Some(c);
                                break;
                            }
                        }
                    }
                }
                if publisher.is_none() {
                    if let Some(ag) = oi.get("agent") {
                        for a in as_slice_or_single(ag) {
                            if let Some(np) = a.get("namePart").and_then(text_of) {
                                let c = clean_marc_field(np);
                                if !c.is_empty() {
                                    publisher = Some(c);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if address.is_none() {
                if let Some(places) = oi.get("place") {
                    for pl in as_slice_or_single(places) {
                        if let Some(pts) = pl.get("placeTerm") {
                            for pt in as_slice_or_single(pts) {
                                let ptype = pt.get("@type").and_then(Value::as_str).unwrap_or("text");
                                let auth = pt.get("@authority").and_then(Value::as_str).unwrap_or("");
                                if ptype == "text" && auth != "marccountry" {
                                    if let Some(txt) = text_of(pt) {
                                        let c = clean_marc_field(txt);
                                        if !c.is_empty() {
                                            address = Some(c);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(dates) = oi.get("dateIssued") {
                for dt in as_slice_or_single(dates) {
                    let is_marc = dt.get("@encoding").and_then(Value::as_str) == Some("marc");
                    if let Some(txt) = text_of(dt) {
                        if let Some(yr) = extract_marc_year(txt) {
                            if is_marc || year.is_none() {
                                year = Some(yr);
                            }
                        }
                    }
                }
            }
            if edition.is_none() {
                if let Some(ed) = oi.get("edition").and_then(text_of) {
                    let c = clean_marc_field(ed);
                    if !c.is_empty() {
                        edition = Some(c);
                    }
                }
            }
        }
    }

    // 4. Pages from physicalDescription.extent
    let mut pages: Option<String> = None;
    if let Some(pds) = m.get("physicalDescription") {
        for pd in as_slice_or_single(pds) {
            if let Some(ext) = pd.get("extent") {
                for ev in as_slice_or_single(ext) {
                    if let Some(txt) = text_of(ev) {
                        if let Some(pg) = extract_pages_from_extent(txt) {
                            pages = Some(pg);
                            break;
                        }
                    }
                }
            }
        }
    }

    // 5. Series & Volume from relatedItem
    let mut series: Option<String> = None;
    let mut volume: Option<String> = None;
    let mut url: Option<String> = None;
    if let Some(rels) = m.get("relatedItem") {
        for rel in as_slice_or_single(rels) {
            if url.is_none() {
                if let Some(u) = rel
                    .get("location")
                    .and_then(|l| l.get("url"))
                    .and_then(text_of)
                {
                    if u.starts_with("http") {
                        url = Some(u.to_string());
                    }
                }
            }
            let is_series = rel.get("@type").and_then(Value::as_str) == Some("series");
            if series.is_none() && (is_series || rel.get("@otherType").is_none()) {
                if let Some(ti) = rel.get("titleInfo") {
                    for t in as_slice_or_single(ti) {
                        if let Some(raw_s) = t.get("title").and_then(text_of) {
                            let part_num = t.get("partNumber").and_then(text_of).or_else(|| {
                                rel.get("part")
                                    .and_then(|p| p.get("detail"))
                                    .and_then(|d| d.get("number"))
                                    .and_then(text_of)
                            });
                            if is_series || raw_s.contains(';') || part_num.is_some() {
                                let (s_opt, v_opt) = split_series_and_volume(raw_s, part_num);
                                if s_opt.is_some() {
                                    series = s_opt;
                                    volume = v_opt;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 6. ISBN from identifier
    let mut isbn: Option<String> = None;
    if let Some(idents) = m.get("identifier") {
        for id_val in as_slice_or_single(idents) {
            let itype = id_val
                .get("@type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            if itype == "isbn" {
                if let Some(txt) = text_of(id_val) {
                    let token = txt.split_whitespace().next().unwrap_or(txt);
                    if let Some(norm) = normalize_isbn(token) {
                        let prefer = isbn.as_ref().is_none_or(|prev| prev.len() < norm.len());
                        if prefer {
                            isbn = Some(norm);
                        }
                    }
                }
            }
        }
    }
    if isbn.is_none() {
        isbn = fallback_isbn.and_then(normalize_isbn);
    }

    Some(BibliographicRecord {
        work_type: WorkType::Book,
        title,
        authors,
        publisher,
        address,
        series,
        volume,
        edition,
        year,
        pages,
        isbn,
        url,
        source: "harvard".to_string(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_harvard_mods_json_with_publisher_place_series_volume_and_marc_dates() {
        let sample = r##"{
          "items": {
            "mods": {
              "titleInfo": { "title": "Partial differential equations /" },
              "name": {
                "@type": "personal",
                "namePart": "Evans, Lawrence C., 1949-",
                "role": { "roleTerm": { "@type": "text", "#text": "author." } }
              },
              "originInfo": [
                {
                  "place": { "placeTerm": { "@type": "code", "@authority": "marccountry", "#text": "riu" } },
                  "dateIssued": { "@encoding": "marc", "#text": "2010" }
                },
                {
                  "place": { "placeTerm": { "@type": "text", "#text": "Providence, R.I. :" } },
                  "publisher": "American Mathematical Society,",
                  "dateIssued": "c2010.",
                  "edition": "2nd ed."
                }
              ],
              "physicalDescription": { "extent": "xxii, 749 p. : ill. ; 26 cm." },
              "relatedItem": {
                "@type": "series",
                "titleInfo": { "title": "Graduate studies in mathematics ; v. 19" }
              },
              "identifier": [
                { "@type": "isbn", "#text": "0821849743" },
                { "@type": "isbn", "#text": "978-0-8218-4974-3 (alk. paper)" }
              ]
            }
          }
        }"##;

        let recs = parse_harvard_items_json(sample, None);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.title, "Partial differential equations");
        assert_eq!(r.authors.len(), 1);
        assert_eq!(r.authors[0].family_name, "Evans");
        assert_eq!(r.authors[0].given_name, "Lawrence C.");
        assert_eq!(r.publisher.as_deref(), Some("American Mathematical Society"));
        assert_eq!(r.address.as_deref(), Some("Providence, R.I."));
        assert_eq!(r.year, Some(2010));
        assert_eq!(r.edition.as_deref(), Some("2nd ed."));
        assert_eq!(r.pages.as_deref(), Some("749"));
        assert_eq!(r.series.as_deref(), Some("Graduate studies in mathematics"));
        assert_eq!(r.volume.as_deref(), Some("19"));
        assert_eq!(r.isbn.as_deref(), Some("9780821849743"));
    }
}
