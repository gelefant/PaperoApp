use crate::db::Database;
use crate::error::{CoreError, Result};
use crate::metadata::extract::{alternate_isbn_form, normalize_isbn};
use crate::metadata::harvard::{
    clean_marc_field, extract_marc_year, extract_pages_from_extent, parse_marc_personal_name,
    split_series_and_volume,
};
use crate::metadata::limiter::{load_http_cache, store_http_cache, SequentialRateLimiter};
use crate::metadata::models::{
    BibliographicAuthor, BibliographicRecord, CandidateMatch, ExtractedIdentifiers, WorkType,
};
use crate::metadata::mrlookup::score_candidate;
use crate::text_norm::normalize_math_text;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use reqwest::Client;
use std::time::Duration;

const LOC_SRU_BASE: &str = "http://lx2.loc.gov:210/LCDB";

/// Library of Congress official SRU Catalog client (`LCDB` MODS v3.8 XML, 100% public domain, no API key).
#[derive(Debug)]
pub struct LocClient {
    http: Client,
    limiter: SequentialRateLimiter,
}

impl LocClient {
    pub fn new(user_agent: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent(user_agent)
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| CoreError::Network(format!("loc client init: {e}")))?;
        Ok(Self {
            http,
            limiter: SequentialRateLimiter::new(Duration::from_millis(300)),
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

        let body = if let Some(cached) = load_http_cache(db, "loc", &cache_key)? {
            if cached.status_code >= 400 {
                return Ok(Vec::new());
            }
            cached.body
        } else {
            let safe_ti = sanitize_cql_term(ti);
            if safe_ti.is_empty() {
                return Ok(Vec::new());
            }
            let cql = if !au.is_empty() {
                let safe_au = sanitize_cql_term(au);
                if safe_au.is_empty() {
                    format!("dc.title=\"{safe_ti}\"")
                } else {
                    format!("dc.title=\"{safe_ti}\" and dc.creator=\"{safe_au}\"")
                }
            } else {
                format!("dc.title=\"{safe_ti}\"")
            };
            let url = format!(
                "{LOC_SRU_BASE}?version=1.1&operation=searchRetrieve&recordSchema=mods&maximumRecords=5&query={}",
                url::form_urlencoded::byte_serialize(cql.as_bytes()).collect::<String>()
            );
            let (status, text) = self.fetch_url(&url).await?;
            if status < 500 && status != 429 {
                store_http_cache(db, "loc", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        let records = parse_loc_sru_mods_xml(&body, None);
        let mut out: Vec<CandidateMatch> = records
            .into_iter()
            .map(|rec| CandidateMatch {
                score: score_candidate(&rec, ids),
                source: "loc".to_string(),
                record: rec,
            })
            .collect();
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(out)
    }

    async fn query_by_isbn(&self, db: &Database, isbn: &str) -> Result<Vec<CandidateMatch>> {
        let cache_key = format!("isbn:{isbn}");
        let body = if let Some(cached) = load_http_cache(db, "loc", &cache_key)? {
            if cached.status_code >= 400 {
                return Ok(Vec::new());
            }
            cached.body
        } else {
            let cql = format!("bath.isbn={isbn}");
            let url = format!(
                "{LOC_SRU_BASE}?version=1.1&operation=searchRetrieve&recordSchema=mods&maximumRecords=4&query={}",
                url::form_urlencoded::byte_serialize(cql.as_bytes()).collect::<String>()
            );
            let (status, text) = self.fetch_url(&url).await?;
            if status < 500 && status != 429 {
                store_http_cache(db, "loc", &cache_key, status, &text)?;
            }
            if status >= 400 {
                return Ok(Vec::new());
            }
            text
        };

        let records = parse_loc_sru_mods_xml(&body, Some(isbn));
        Ok(records
            .into_iter()
            .map(|rec| CandidateMatch {
                score: 0.985,
                source: "loc".to_string(),
                record: rec,
            })
            .collect())
    }

    async fn fetch_url(&self, url: &str) -> Result<(u16, String)> {
        self.limiter.acquire().await;
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| CoreError::Network(format!("loc request: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .await
            .map_err(|e| CoreError::Network(format!("loc read body: {e}")))?;
        Ok((status, text))
    }
}

fn sanitize_cql_term(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '"' | '\\' | '?' | '*' | '^' => ' ',
            _ => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn local_tag_name(raw: &str) -> String {
    raw.rsplit_once(':')
        .map(|(_, local)| local)
        .unwrap_or(raw)
        .to_string()
}

fn attr_val(e: &BytesStart<'_>, name: &str) -> Option<String> {
    for attr in e.attributes().flatten() {
        let k = attr.key.as_ref();
        let local_k = k.rsplit_once(':').map(|(_, l)| l).unwrap_or(k);
        if local_k == name {
            let val_str = attr.value.as_ref();
            return Some(
                quick_xml::escape::unescape(val_str)
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|_| val_str.to_string()),
            );
        }
    }
    None
}

#[derive(Default)]
struct ModsBuilder {
    non_sort: String,
    main_title: String,
    subtitle: String,
    got_primary_title: bool,
    in_primary_title_info: bool,

    in_personal_name: bool,
    cur_name_parts: Vec<String>,
    cur_name_is_editor: bool,
    authors: Vec<BibliographicAuthor>,

    in_series_related: bool,
    series_title: String,
    series_part_num: String,

    publisher: Option<String>,
    address: Option<String>,
    year: Option<i64>,
    edition: Option<String>,
    pages: Option<String>,
    isbn: Option<String>,
    lccn: Option<String>,

    cur_place_is_text: bool,
    cur_date_is_marc: bool,
    cur_ident_type: String,
    cur_namepart_is_date: bool,
}

impl ModsBuilder {
    fn finish(self, fallback_isbn: Option<&str>) -> Option<BibliographicRecord> {
        let base_title = clean_marc_field(&self.main_title);
        if base_title.is_empty() {
            return None;
        }
        let non_sort = self.non_sort.trim();
        let mut title = if non_sort.is_empty() {
            base_title
        } else {
            format!("{non_sort} {base_title}")
        };
        let sub = clean_marc_field(&self.subtitle);
        let sub_trimmed = sub.trim_end_matches('.').trim();
        if !sub_trimmed.is_empty()
            && !normalize_math_text(&title).contains(&normalize_math_text(sub_trimmed))
        {
            title = format!("{}: {}", title.trim_end_matches('.'), sub_trimmed);
        }
        title = title.trim_end_matches('.').trim().to_string();

        let (series, volume) = if !self.series_title.trim().is_empty() {
            let explicit_v = (!self.series_part_num.trim().is_empty())
                .then_some(self.series_part_num.as_str());
            split_series_and_volume(&self.series_title, explicit_v)
        } else {
            (None, None)
        };

        let isbn = self.isbn.or_else(|| fallback_isbn.and_then(normalize_isbn));
        let url = self
            .lccn
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|l| format!("https://lccn.loc.gov/{}", l.replace(' ', "")));

        Some(BibliographicRecord {
            work_type: WorkType::Book,
            title,
            authors: self.authors,
            publisher: self.publisher,
            address: self.address,
            series,
            volume,
            edition: self.edition,
            year: self.year,
            pages: self.pages,
            isbn,
            url,
            source: "loc".to_string(),
            ..Default::default()
        })
    }
}

pub fn parse_loc_sru_mods_xml(xml: &str, fallback_isbn: Option<&str>) -> Vec<BibliographicRecord> {
    let mut reader = Reader::from_str(xml);

    let mut stack: Vec<String> = Vec::with_capacity(16);
    let mut cur: Option<ModsBuilder> = None;
    let mut cur_text = String::new();
    let mut out = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                cur_text.clear();
                let tag = local_tag_name(e.name().as_ref());
                if tag == "mods" && cur.is_none() {
                    cur = Some(ModsBuilder::default());
                    stack.clear();
                    stack.push(tag);
                    continue;
                }
                if let Some(ref mut b) = cur {
                    let depth = stack.len();
                    if depth == 1 && tag == "titleInfo" {
                        let ttype = attr_val(&e, "type");
                        if ttype.is_none() && !b.got_primary_title {
                            b.in_primary_title_info = true;
                        }
                    } else if depth == 1 && tag == "name" {
                        let ntype = attr_val(&e, "type").unwrap_or_else(|| "personal".into());
                        if ntype == "personal" {
                            b.in_personal_name = true;
                            b.cur_name_parts.clear();
                            b.cur_name_is_editor = false;
                        }
                    } else if b.in_personal_name && tag == "namePart" {
                        b.cur_namepart_is_date =
                            attr_val(&e, "type").as_deref() == Some("date");
                    } else if depth == 1 && tag == "relatedItem" {
                        if attr_val(&e, "type").as_deref() == Some("series")
                            && b.series_title.is_empty()
                        {
                            b.in_series_related = true;
                        }
                    } else if tag == "placeTerm" {
                        let ptype = attr_val(&e, "type").unwrap_or_else(|| "text".into());
                        let auth = attr_val(&e, "authority").unwrap_or_default();
                        b.cur_place_is_text = ptype == "text" && auth != "marccountry";
                    } else if tag == "dateIssued" {
                        b.cur_date_is_marc =
                            attr_val(&e, "encoding").as_deref() == Some("marc");
                    } else if depth == 1 && tag == "identifier" {
                        b.cur_ident_type = attr_val(&e, "type")
                            .unwrap_or_default()
                            .to_ascii_lowercase();
                    }
                    stack.push(tag);
                }
            }
            Ok(Event::Text(t)) => {
                if cur.is_some() {
                    cur_text.push_str(t.as_ref());
                }
            }
            Ok(Event::GeneralRef(gr)) => {
                if cur.is_some() {
                    if let Ok(Some(ch)) = gr.resolve_char_ref() {
                        cur_text.push(ch);
                    } else {
                        match gr.as_ref() {
                            "amp" => cur_text.push('&'),
                            "lt" => cur_text.push('<'),
                            "gt" => cur_text.push('>'),
                            "quot" => cur_text.push('"'),
                            "apos" => cur_text.push('\''),
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::End(e)) => {
                let tag = local_tag_name(e.name().as_ref());
                if let Some(mut b) = cur.take() {
                    let s = cur_text.trim();
                    if !s.is_empty() {
                        let leaf = stack.last().map(String::as_str).unwrap_or("");
                        let parent = stack
                            .len()
                            .checked_sub(2)
                            .and_then(|i| stack.get(i))
                            .map(String::as_str)
                            .unwrap_or("");

                        if b.in_primary_title_info && parent == "titleInfo" {
                            match leaf {
                                "nonSort" => b.non_sort = s.to_string(),
                                "title" if b.main_title.is_empty() => b.main_title = s.to_string(),
                                "subTitle" if b.subtitle.is_empty() => b.subtitle = s.to_string(),
                                _ => {}
                            }
                        } else if b.in_personal_name {
                            if leaf == "namePart" && !b.cur_namepart_is_date {
                                if s.to_ascii_lowercase().contains("editor") {
                                    b.cur_name_is_editor = true;
                                }
                                b.cur_name_parts.push(s.to_string());
                            } else if leaf == "roleTerm" {
                                let low = s.to_ascii_lowercase();
                                if low.contains("editor") || low == "ed." || low == "edt" {
                                    b.cur_name_is_editor = true;
                                }
                            }
                        } else if b.in_series_related {
                            if leaf == "title" && b.series_title.is_empty() {
                                b.series_title = s.to_string();
                            } else if (leaf == "partNumber" || leaf == "number")
                                && b.series_part_num.is_empty()
                            {
                                b.series_part_num = s.to_string();
                            }
                        } else if stack.iter().any(|p| p == "originInfo") {
                            match leaf {
                                "publisher" if b.publisher.is_none() => {
                                    let c = clean_marc_field(s);
                                    if !c.is_empty() {
                                        b.publisher = Some(c);
                                    }
                                }
                                "namePart" if parent == "agent" && b.publisher.is_none() => {
                                    let c = clean_marc_field(s);
                                    if !c.is_empty() {
                                        b.publisher = Some(c);
                                    }
                                }
                                "placeTerm" if b.cur_place_is_text && b.address.is_none() => {
                                    let c = clean_marc_field(s);
                                    if !c.is_empty() {
                                        b.address = Some(c);
                                    }
                                }
                                "dateIssued" => {
                                    if let Some(yr) = extract_marc_year(s) {
                                        if b.cur_date_is_marc || b.year.is_none() {
                                            b.year = Some(yr);
                                        }
                                    }
                                }
                                "edition" if b.edition.is_none() => {
                                    let c = clean_marc_field(s);
                                    if !c.is_empty() {
                                        b.edition = Some(c);
                                    }
                                }
                                _ => {}
                            }
                        } else if leaf == "extent"
                            && parent == "physicalDescription"
                            && b.pages.is_none()
                        {
                            b.pages = extract_pages_from_extent(s);
                        } else if leaf == "identifier" && stack.len() == 2 {
                            if b.cur_ident_type == "isbn" {
                                let tok = s.split_whitespace().next().unwrap_or(s);
                                if let Some(norm) = normalize_isbn(tok) {
                                    let prefer = b
                                        .isbn
                                        .as_ref()
                                        .is_none_or(|prev| prev.len() < norm.len());
                                    if prefer {
                                        b.isbn = Some(norm);
                                    }
                                }
                            } else if b.cur_ident_type == "lccn" && b.lccn.is_none() {
                                b.lccn = Some(s.to_string());
                            }
                        }
                    }
                    cur_text.clear();

                    if tag == "mods" && stack.len() == 1 {
                        stack.clear();
                        if let Some(rec) = b.finish(fallback_isbn) {
                            out.push(rec);
                        }
                        continue;
                    }
                    let _ = stack.pop();
                    if stack.len() == 1 && tag == "titleInfo" && b.in_primary_title_info {
                        b.in_primary_title_info = false;
                        if !b.main_title.is_empty() {
                            b.got_primary_title = true;
                        }
                    } else if stack.len() == 1 && tag == "name" && b.in_personal_name {
                        b.in_personal_name = false;
                        let combined = b.cur_name_parts.join(" ");
                        if let Some(au) = parse_marc_personal_name(&combined, b.cur_name_is_editor)
                        {
                            if !b.authors.iter().any(|ex| {
                                ex.family_name.eq_ignore_ascii_case(&au.family_name)
                                    && ex.given_name.eq_ignore_ascii_case(&au.given_name)
                            }) {
                                b.authors.push(au);
                            }
                        }
                    } else if stack.len() == 1 && tag == "relatedItem" && b.in_series_related {
                        b.in_series_related = false;
                    } else if tag == "namePart" {
                        b.cur_namepart_is_date = false;
                    } else if tag == "placeTerm" {
                        b.cur_place_is_text = false;
                    } else if tag == "dateIssued" {
                        b.cur_date_is_marc = false;
                    } else if tag == "identifier" {
                        b.cur_ident_type.clear();
                    }
                    cur = Some(b);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_loc_sru_mods_xml_with_agent_publisher_series_volume_and_pages() {
        let xml = r#"<?xml version="1.0"?>
        <zs:searchRetrieveResponse xmlns:zs="http://www.loc.gov/zing/srw/">
          <zs:records>
            <zs:record>
              <zs:recordData>
                <mods xmlns="http://www.loc.gov/mods/v3" version="3.8">
                  <titleInfo>
                    <title>Algebraic geometry</title>
                  </titleInfo>
                  <name type="personal" usage="primary">
                    <namePart>Hartshorne, Robin.</namePart>
                  </name>
                  <originInfo>
                    <place><placeTerm type="code" authority="marccountry">nyu</placeTerm></place>
                    <dateIssued encoding="marc">1977</dateIssued>
                    <place><placeTerm type="text">New York</placeTerm></place>
                    <agent><namePart>Springer-Verlag</namePart></agent>
                    <dateIssued>c1977</dateIssued>
                  </originInfo>
                  <physicalDescription>
                    <extent>xvi, 496 p. ; 25 cm.</extent>
                  </physicalDescription>
                  <relatedItem type="series">
                    <titleInfo>
                      <title>Graduate texts in mathematics</title>
                      <partNumber>52</partNumber>
                    </titleInfo>
                  </relatedItem>
                  <identifier type="isbn">0387902449</identifier>
                  <identifier type="lccn">77001177</identifier>
                </mods>
              </zs:recordData>
            </zs:record>
          </zs:records>
        </zs:searchRetrieveResponse>"#;

        let recs = parse_loc_sru_mods_xml(xml, None);
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.title, "Algebraic geometry");
        assert_eq!(r.authors.len(), 1);
        assert_eq!(r.authors[0].family_name, "Hartshorne");
        assert_eq!(r.authors[0].given_name, "Robin");
        assert_eq!(r.publisher.as_deref(), Some("Springer-Verlag"));
        assert_eq!(r.address.as_deref(), Some("New York"));
        assert_eq!(r.year, Some(1977));
        assert_eq!(r.pages.as_deref(), Some("496"));
        assert_eq!(r.series.as_deref(), Some("Graduate texts in mathematics"));
        assert_eq!(r.volume.as_deref(), Some("52"));
        assert_eq!(r.isbn.as_deref(), Some("0387902449"));
        assert_eq!(r.url.as_deref(), Some("https://lccn.loc.gov/77001177"));
    }
}
