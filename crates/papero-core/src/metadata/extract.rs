use crate::extract::types::{ExtractedPage, NormalizedRect};
use crate::metadata::models::{
    clean_author_surname, extract_arxiv_from_doi, normalize_doi, normalize_mr_digits,
    ExtractedIdentifiers,
};
use crate::text_norm::{clean_ot1_and_ligatures_preserve_case, normalize_math_text};
use regex::Regex;
use std::sync::OnceLock as StdOnceLock;

fn doi_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)\b10\.\d{4,9}/[^\s"'\]]+"#).unwrap())
}

fn doi_space_slash_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b(10\.\d{4,9})\s*/\s*").unwrap())
}

fn doi_linewrap_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(\b10\.\d{4,9}/[^\s\r\n]*[-/])\r?\n\s*([A-Za-z0-9(][^\s\r\n]+)").unwrap()
    })
}

fn jstor_stable_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)jstor\.org/stable/(\d{5,10})\b").unwrap())
}

fn pii_sici_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:PII[:\s]*)?(S\d{4}-\d{3}[\dXx]\(\d{2}\)\d{5}-[\dXx])\b").unwrap()
    })
}

fn pii_unpunctuated_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:\bPII[:\s]*|1-s2\.0-)([SB]\d{7}[\dXx]\d{7,10}[\dXx])\b").unwrap()
    })
}

/// Extracts and normalizes the first valid DOI from a text block, handling Unicode hyphens,
/// spaces around `/`, line-wrapped DOIs, and Elsevier/Wiley SICI parentheses.
pub fn extract_doi_from_text(blob: &str) -> Option<String> {
    let mut pre = String::with_capacity(blob.len());
    for ch in blob.chars() {
        match ch {
            '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' => {}
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}' => {
                pre.push('-');
            }
            _ => pre.push(ch),
        }
    }
    let slash_joined = doi_space_slash_re().replace_all(&pre, "$1/");
    let unwrapped = doi_linewrap_re().replace_all(&slash_joined, "$1$2");
    for m in doi_re().find_iter(&unwrapped) {
        if let Some(norm) = normalize_doi(m.as_str()) {
            return Some(norm);
        }
    }
    None
}

/// Extracts Elsevier PII (and corresponding 10.1016 DOI when in punctuated SICI form).
fn extract_pii_and_doi_from_text(blob: &str) -> (Option<String>, Option<String>) {
    if let Some(c) = pii_sici_re().captures(blob) {
        if let Some(m) = c.get(1) {
            let sici = m.as_str();
            let clean_pii: String = sici
                .chars()
                .filter(|ch| ch.is_ascii_alphanumeric())
                .map(|ch| ch.to_ascii_uppercase())
                .collect();
            let doi = normalize_doi(&format!("10.1016/{sici}"));
            return (Some(clean_pii), doi);
        }
    }
    if let Some(c) = pii_unpunctuated_re().captures(blob) {
        if let Some(m) = c.get(1) {
            return (Some(m.as_str().to_ascii_uppercase()), None);
        }
    }
    (None, None)
}

fn arxiv_new_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:\barXiv:\s*|arxiv\.org/(?:abs|pdf)/)(\d{4}\.\d{4,5})(v\d+)?\b").unwrap()
    })
}

fn arxiv_old_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:\barXiv:\s*|arxiv\.org/(?:abs|pdf)/)([a-z-]+(?:\.[A-Z]{2})?/\d{7})(v\d+)?\b",
        )
        .unwrap()
    })
}

fn isbn_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bISBN(?:-1[03])?[:\s]*((?:97[89][-\s]?)?(?:\d[-\s]?){9}[\dXx])\b")
            .unwrap()
    })
}

fn mr_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bMR(?:\s*number)?[:#\s]*0*(\d{5,7})\b").unwrap())
}

fn zbl_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:Zbl|zbMATH)(?:\s*number)?[:#\s]*(\d{4}\.\d{5}|\d{3,4}\.\d{5})\b")
            .unwrap()
    })
}

fn year_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(19[0-9]{2}|20[0-2][0-9])\b").unwrap())
}

fn msc_year_noise_re() -> &'static Regex {
    static RE: StdOnceLock<Regex> = StdOnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:\b(?:1970|1980|1991|2000|2010|2020)\s+mathematics\s+subject\s+classification\b|\b(?:msc|ams(?:\s*\(mos\))?\s*subject\s*classifications?)\s*\(?\s*(?:1970|1980|1991|2000|2010|2020)\s*\)?)",
        )
        .unwrap()
    })
}

/// Validates and normalizes ISBN-10 or ISBN-13 (digits/X only).
pub fn normalize_isbn(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'x' || *c == 'X')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if cleaned.len() == 13 && (cleaned.starts_with("978") || cleaned.starts_with("979")) {
        let mut sum: u32 = 0;
        for (i, ch) in cleaned.chars().enumerate() {
            let d = ch.to_digit(10)?;
            sum += if i % 2 == 0 { d } else { d * 3 };
        }
        if sum.is_multiple_of(10) {
            return Some(cleaned);
        }
    } else if cleaned.len() == 10 {
        let mut sum: u32 = 0;
        for (i, ch) in cleaned.chars().enumerate() {
            let d = if i == 9 && ch == 'X' {
                10
            } else {
                ch.to_digit(10)?
            };
            sum += (10 - i as u32) * d;
        }
        if sum.is_multiple_of(11) {
            return Some(cleaned);
        }
    }
    None
}

/// Converts a normalized ISBN-13 (`978...`) to its equivalent ISBN-10, or an ISBN-10 to its
/// equivalent `978` ISBN-13, so library catalogs indexing only one form still match cleanly.
pub fn alternate_isbn_form(isbn: &str) -> Option<String> {
    let norm = normalize_isbn(isbn)?;
    if norm.len() == 13 && norm.starts_with("978") {
        let core = &norm[3..12];
        let mut sum: u32 = 0;
        for (i, ch) in core.chars().enumerate() {
            sum += (10 - i as u32) * ch.to_digit(10)?;
        }
        let rem = (11 - (sum % 11)) % 11;
        let check = if rem == 10 {
            'X'
        } else {
            char::from_digit(rem, 10)?
        };
        let mut out = String::with_capacity(10);
        out.push_str(core);
        out.push(check);
        Some(out)
    } else if norm.len() == 10 {
        let prefix_core = format!("978{}", &norm[..9]);
        let mut sum: u32 = 0;
        for (i, ch) in prefix_core.chars().enumerate() {
            let d = ch.to_digit(10)?;
            sum += if i % 2 == 0 { d } else { d * 3 };
        }
        let check = (10 - (sum % 10)) % 10;
        Some(format!("{prefix_core}{check}"))
    } else {
        None
    }
}

/// Extracts bibliographic identifiers and first-page title/author/year heuristics from
/// optional XMP XML, PDF/DjVu embedded doc title/author, first few pages, last page, and filename.
pub fn extract_identifiers_and_heuristics(
    xmp_xml: Option<&str>,
    doc_title: Option<&str>,
    doc_author: Option<&str>,
    first_pages_text: &[&str],
    last_page_text: Option<&str>,
    page_count: u32,
    filename_stem: Option<&str>,
) -> ExtractedIdentifiers {
    extract_identifiers_and_heuristics_with_layout(
        xmp_xml,
        doc_title,
        doc_author,
        first_pages_text,
        last_page_text,
        page_count,
        filename_stem,
        None,
    )
}

/// Layout-aware identifier and front-matter extractor that also uses page-1 line bounding boxes
/// and font heights (`page1_layout`) when available to reconstruct multi-line titles and authors.
#[allow(clippy::too_many_arguments)]
pub fn extract_identifiers_and_heuristics_with_layout(
    xmp_xml: Option<&str>,
    doc_title: Option<&str>,
    doc_author: Option<&str>,
    first_pages_text: &[&str],
    last_page_text: Option<&str>,
    page_count: u32,
    filename_stem: Option<&str>,
    page1_layout: Option<&ExtractedPage>,
) -> ExtractedIdentifiers {
    extract_identifiers_and_heuristics_with_front_pages(
        xmp_xml,
        doc_title,
        doc_author,
        first_pages_text,
        last_page_text,
        page_count,
        filename_stem,
        page1_layout,
        None,
    )
}

/// Full front-matter extractor using both page 1 and page 2 layout spans so repository cover sheets
/// (JSTOR, NUMDAM, EuDML, HAL, ResearchGate, SSRN) seamlessly promote page 2's real article header.
#[allow(clippy::too_many_arguments)]
pub fn extract_identifiers_and_heuristics_with_front_pages(
    xmp_xml: Option<&str>,
    doc_title: Option<&str>,
    doc_author: Option<&str>,
    first_pages_text: &[&str],
    last_page_text: Option<&str>,
    page_count: u32,
    filename_stem: Option<&str>,
    page1_layout: Option<&ExtractedPage>,
    page2_layout: Option<&ExtractedPage>,
) -> ExtractedIdentifiers {
    let has_book_structure = page_count >= 90
        && first_pages_text.iter().take(4).any(|p| {
            let low = p.to_ascii_lowercase();
            low.contains("table of contents")
                || low.contains("\ncontents\n")
                || low.contains("chapter 1")
                || low.contains("preface")
        });

    let mut out = ExtractedIdentifiers {
        looks_like_book: page_count >= 120 || has_book_structure,
        ..Default::default()
    };

    let p1_is_cover = first_pages_text
        .first()
        .is_some_and(|p| is_repository_cover_page(p) || is_book_half_title_or_boilerplate_page(p))
        && first_pages_text.len() >= 2;
    let id_page_limit = if p1_is_cover { 3 } else { 2 };

    // Article-level identifiers (DOI, PII, arXiv, MR, Zbl) are ONLY searched in embedded metadata and
    // front pages BEFORE any References/Bibliography section (never in bibliography/end pages, where
    // cited papers' DOIs/arXiv IDs/MR numbers would cause false matches).
    let mut article_id_blobs: Vec<String> = Vec::new();
    if let Some(xmp) = xmp_xml {
        article_id_blobs.push(xmp.to_string());
    }
    if let Some(dt) = doc_title {
        article_id_blobs.push(dt.to_string());
    }
    if let Some(da) = doc_author {
        article_id_blobs.push(da.to_string());
    }
    for p in first_pages_text.iter().take(id_page_limit) {
        let cleaned = strip_references_and_citations(p);
        if !cleaned.is_empty() {
            article_id_blobs.push(cleaned);
        }
    }

    let mut jstor_fallback_doi: Option<String> = None;

    for blob in &article_id_blobs {
        if out.doi.is_none() {
            out.doi = extract_doi_from_text(blob);
        }
        if out.pii.is_none() {
            let (pii_hit, pii_doi) = extract_pii_and_doi_from_text(blob);
            if pii_hit.is_some() {
                out.pii = pii_hit;
            }
            if out.doi.is_none() && pii_doi.is_some() {
                out.doi = pii_doi;
            }
        }
        if jstor_fallback_doi.is_none() {
            if let Some(c) = jstor_stable_re().captures(blob) {
                if let Some(id_m) = c.get(1) {
                    jstor_fallback_doi = Some(format!("10.2307/{}", id_m.as_str()));
                }
            }
        }
        if out.arxiv_id.is_none() {
            if let Some(c) = arxiv_new_re().captures(blob) {
                out.arxiv_id = c.get(1).map(|m| m.as_str().to_string());
                out.arxiv_version = c.get(2).map(|m| m.as_str().to_string());
            } else if let Some(c) = arxiv_old_re().captures(blob) {
                out.arxiv_id = c.get(1).map(|m| m.as_str().to_ascii_lowercase());
                out.arxiv_version = c.get(2).map(|m| m.as_str().to_string());
            }
        }
        if out.mr_number.is_none() {
            if let Some(c) = mr_re().captures(blob) {
                if let Some(d) = c.get(1) {
                    out.mr_number = normalize_mr_digits(d.as_str());
                }
            }
        }
        if out.zbl_number.is_none() {
            if let Some(c) = zbl_re().captures(blob) {
                out.zbl_number = c.get(1).map(|m| m.as_str().to_string());
            }
        }
    }

    if out.doi.is_none() {
        out.doi = jstor_fallback_doi;
    }

    let fn_parsed = filename_stem.map(parse_filename_metadata).unwrap_or_default();
    if out.doi.is_none() {
        out.doi = fn_parsed.doi;
    }
    if out.pii.is_none() {
        out.pii = fn_parsed.pii;
    }
    if out.arxiv_id.is_none() {
        out.arxiv_id = fn_parsed.arxiv_id;
        if out.arxiv_version.is_none() {
            out.arxiv_version = fn_parsed.arxiv_version;
        }
    }
    if out.arxiv_id.is_none() {
        if let Some(ref d) = out.doi {
            out.arxiv_id = extract_arxiv_from_doi(d);
        }
    }

    // ISBNs may appear on copyright pages (pages 1–6) or publisher colophon on the last page,
    // still excluding bibliography/citation lines and series-advertisement catalogs.
    if let Some(xmp) = xmp_xml {
        out.isbn = extract_isbn_from_page_blob(xmp, true);
    }
    if out.isbn.is_none() {
        let mut front_clean: Vec<String> = first_pages_text
            .iter()
            .take(6)
            .map(|p| strip_references_and_citations(p))
            .collect();
        if let Some(last) = last_page_text {
            front_clean.push(strip_references_and_citations(last));
        }
        // Pass 1: copyright/colophon pages (most reliable for THIS volume's ISBN)
        for blob in &front_clean {
            if is_copyright_or_colophon_blob(blob) && !looks_like_series_catalog_blob(blob) {
                if let Some(norm) = extract_isbn_from_page_blob(blob, false) {
                    out.isbn = Some(norm);
                    break;
                }
            }
        }
        // Pass 2: other non-catalog front/end pages
        if out.isbn.is_none() {
            for blob in &front_clean {
                if !looks_like_series_catalog_blob(blob) {
                    if let Some(norm) = extract_isbn_from_page_blob(blob, false) {
                        out.isbn = Some(norm);
                        break;
                    }
                }
            }
        }
        if out.isbn.is_some() && page_count >= 50 {
            out.looks_like_book = true;
        }
    }
    if out.isbn.is_none() {
        if let Some(isbn) = fn_parsed.isbn {
            out.isbn = Some(isbn);
            out.looks_like_book = true;
        }
    }
    out.filename_keywords = fn_parsed.keywords;

    // XMP dc:title / dc:creator fallback + doc_title / doc_author
    let xmp_title = xmp_xml
        .and_then(|x| extract_xmp_tag(x, "dc:title"))
        .filter(|t| t.len() >= 4 && !looks_like_garbage_title(t));
    let xmp_creator = xmp_xml
        .and_then(|x| extract_xmp_tag(x, "dc:creator"))
        .filter(|a| !looks_like_garbage_author(a));

    let stripped_doc_title = doc_title.map(strip_inline_markup_tags);
    let cleaned_doc_title = stripped_doc_title
        .as_deref()
        .map(str::trim)
        .filter(|s| s.len() >= 4 && !looks_like_garbage_title(s));
    let stripped_doc_author = doc_author.map(strip_inline_markup_tags);
    let cleaned_doc_author = stripped_doc_author
        .as_deref()
        .map(str::trim)
        .filter(|s| s.len() >= 3 && !looks_like_garbage_author(s));

    // Layout-based extraction from page 1 and (when page 1 is a repository cover sheet or barren) page 2
    let (p1_layout_title, p1_layout_authors) = if p1_is_cover {
        (None, Vec::new())
    } else {
        page1_layout
            .filter(|p| !p.spans.is_empty())
            .map(extract_layout_title_and_authors)
            .unwrap_or((None, Vec::new()))
    };

    let (p1_h_title, p1_h_authors, p1_h_year) = if p1_is_cover {
        (None, Vec::new(), None)
    } else {
        first_pages_text
            .first()
            .map(|p| heuristic_first_page(p))
            .unwrap_or((None, Vec::new(), None))
    };

    let p2_is_boilerplate = first_pages_text
        .get(1)
        .is_some_and(|p| is_book_half_title_or_boilerplate_page(p));

    let (p2_layout_title, p2_layout_authors) = if p2_is_boilerplate {
        (None, Vec::new())
    } else {
        page2_layout
            .filter(|p| !p.spans.is_empty())
            .map(extract_layout_title_and_authors)
            .unwrap_or((None, Vec::new()))
    };

    let (p2_h_title, p2_h_authors, p2_h_year) = if !p2_is_boilerplate
        && (p1_is_cover || (p1_layout_title.is_none() && p1_h_title.is_none()))
    {
        first_pages_text
            .get(1)
            .map(|p| heuristic_first_page(p))
            .unwrap_or((None, Vec::new(), None))
    } else {
        (None, Vec::new(), None)
    };

    let cover_authors = if p1_is_cover {
        first_pages_text
            .first()
            .map(|p| extract_cover_page_authors(p))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let p1_title = p1_layout_title.or(p1_h_title);
    let p2_title = p2_layout_title.or(p2_h_title);
    let mut page_title = if p1_is_cover {
        p2_title.clone().or(p1_title.clone())
    } else {
        p1_title.clone().or(p2_title.clone())
    };

    // Many books have Page 1 = series half-title ("Graduate Texts in Mathematics 142 / Editorial Board"),
    // Page 2–4 = series volume list / colophon / blank, and Page 3–6 = actual book title page.
    let mut p34_h_authors: Vec<String> = Vec::new();
    let mut p34_h_year: Option<i64> = None;
    if page_title.is_none() {
        for &pg_txt in first_pages_text.iter().skip(2).take(4) {
            if is_book_half_title_or_boilerplate_page(pg_txt) {
                continue;
            }
            let (ti, aus, yr) = heuristic_first_page(pg_txt);
            if ti.is_some() {
                page_title = ti;
                p34_h_authors = aus;
                p34_h_year = yr;
                break;
            }
        }
    }

    let raw_embedded_title = cleaned_doc_title.map(ToOwned::to_owned).or(xmp_title);
    // Zotero-style check: only trust embedded PDF /Title or XMP title if its words actually appear
    // in the front pages (prevents stale LaTeX template titles or publisher boilerplate from hijacking lookup).
    let embedded_title = raw_embedded_title
        .filter(|emb| text_confirms_embedded_title(emb, first_pages_text));

    let fn_title = fn_parsed.title;
    out.heuristic_title =
        choose_best_title(embedded_title.clone(), page_title.clone()).or_else(|| fn_title.clone());

    // Build deduplicated alternative title hypotheses for multi-pass waterfall fallback
    let mut alt_titles: Vec<String> = Vec::new();
    let mut push_alt = |cand: Option<String>| {
        if let Some(c) = cand {
            let trimmed = c.trim();
            if trimmed.len() < 6 || looks_like_garbage_title(trimmed) {
                return;
            }
            let c_norm = normalize_math_text(trimmed);
            if c_norm.split_whitespace().count() < 2 {
                return;
            }
            if out
                .heuristic_title
                .as_deref()
                .is_some_and(|primary| normalize_math_text(primary) == c_norm)
            {
                return;
            }
            if alt_titles.iter().any(|ex| normalize_math_text(ex) == c_norm) {
                return;
            }
            alt_titles.push(trimmed.to_string());
        }
    };

    push_alt(page_title);
    push_alt(embedded_title);
    if !p1_is_cover {
        push_alt(p2_title);
    }
    push_alt(fn_title);
    if let Some(ref primary) = out.heuristic_title {
        push_alt(extract_pre_colon_main_title(primary));
        push_alt(extract_prose_core_title(primary));
    }
    alt_titles.truncate(4);
    out.alt_heuristic_titles = alt_titles;

    // Collect authors in priority order while deduplicating surnames
    let mut authors_acc: Vec<String> = Vec::new();
    if let Some(da) =
        cleaned_doc_author.filter(|a| text_confirms_embedded_author(a, first_pages_text))
    {
        for a in split_author_list(da) {
            push_unique_author(&mut authors_acc, a);
        }
    }
    if authors_acc.is_empty() {
        if let Some(xc) = xmp_creator
            .as_deref()
            .filter(|a| text_confirms_embedded_author(a, first_pages_text))
        {
            for a in split_author_list(xc) {
                push_unique_author(&mut authors_acc, a);
            }
        }
    }
    let primary_layout_authors = if p1_is_cover {
        p2_layout_authors
    } else if !p1_layout_authors.is_empty() {
        p1_layout_authors
    } else {
        p2_layout_authors
    };
    for a in primary_layout_authors {
        push_unique_author(&mut authors_acc, a);
    }
    if authors_acc.is_empty() {
        let primary_h_authors = if p1_is_cover {
            if !p2_h_authors.is_empty() {
                p2_h_authors
            } else {
                p34_h_authors
            }
        } else if !p1_h_authors.is_empty() {
            p1_h_authors
        } else if !p2_h_authors.is_empty() {
            p2_h_authors
        } else {
            p34_h_authors
        };
        for a in primary_h_authors {
            push_unique_author(&mut authors_acc, a);
        }
    }
    if authors_acc.is_empty() {
        for a in cover_authors {
            push_unique_author(&mut authors_acc, a);
        }
    }
    if authors_acc.is_empty() {
        if let Some(fa) = fn_parsed.author {
            push_unique_author(&mut authors_acc, fa);
        }
    }

    out.heuristic_first_author = authors_acc.first().cloned().or_else(|| {
        cleaned_doc_author
            .filter(|a| text_confirms_embedded_author(a, first_pages_text))
            .map(first_author_from_raw)
            .or_else(|| {
                xmp_creator
                    .as_deref()
                    .filter(|a| text_confirms_embedded_author(a, first_pages_text))
                    .map(first_author_from_raw)
            })
    });
    out.heuristic_authors = authors_acc;

    let arxiv_inferred_year = out.arxiv_id.as_deref().and_then(infer_year_from_arxiv_id);

    out.heuristic_year = (if p1_is_cover {
        p2_h_year.or(p34_h_year)
    } else {
        p1_h_year.or(p2_h_year).or(p34_h_year)
    })
    .or_else(|| {
        first_pages_text
            .iter()
            .take(4)
            .find_map(|p| extract_clean_year(p))
    })
    .or(fn_parsed.year)
    .or(arxiv_inferred_year);

    out
}

fn is_repository_cover_page(page_text: &str) -> bool {
    let low = page_text.to_ascii_lowercase();
    if low.contains("1. introduction") || low.contains("abstract.") {
        return false;
    }
    // JSTOR
    if low.contains("jstor")
        && (low.contains("stable url")
            || low.contains("your use of the jstor archive")
            || low.contains("collaborating with jstor")
            || low.contains("jstor.org/stable/"))
    {
        return true;
    }
    // NUMDAM / Cedram
    if low.contains("numdam")
        && (low.contains("accès aux archives")
            || low.contains("conditions générales")
            || low.contains("numdam.org"))
    {
        return true;
    }
    // EuDML / DigiZeitschriften
    if (low.contains("eudml.org") || low.contains("digizeitschriften"))
        && (low.contains("terms and conditions")
            || low.contains("nutzungsbedingungen")
            || page_text.lines().count() <= 32)
    {
        return true;
    }
    // HAL open archive cover sheet
    if (low.contains("hal.archives-ouvertes") || low.contains("hal.science") || low.contains("hal id"))
        && low.contains("submitted on")
    {
        return true;
    }
    // ResearchGate cover sheet
    if low.contains("see discussions, stats, and author profiles for this publication") {
        return true;
    }
    // SSRN cover sheet
    if low.contains("electronic copy available at")
        && low.contains("ssrn.com")
        && page_text.split_whitespace().count() < 220
    {
        return true;
    }
    false
}

fn is_copyright_or_colophon_blob(blob: &str) -> bool {
    let low = blob.to_lowercase();
    low.contains("copyright")
        || low.contains('©')
        || low.contains("all rights reserved")
        || low.contains("tutti i diritti riservati")
        || low.contains("library of congress")
        || low.contains("cataloging-in-publication")
        || low.contains("finito di stampare")
        || low.contains("printed in ")
        || low.contains("stampato in ")
}

fn looks_like_series_catalog_blob(blob: &str) -> bool {
    let low = blob.to_lowercase().replace(['’', '‘'], "'");
    if low.contains("nella stessa collana")
        || low.contains("volumi pubblicati")
        || low.contains("volumi usciti")
        || low.contains("elenco dei volumi")
        || low.contains("piano dell'opera")
        || low.contains("titles in this series")
        || low.contains("other titles in this series")
        || low.contains("volumes in this series")
        || low.contains("dans la même collection")
    {
        return true;
    }
    let mut distinct_isbns: Vec<String> = Vec::new();
    for cap in isbn_re().captures_iter(blob) {
        if let Some(norm) = normalize_isbn(cap.get(1).map(|m| m.as_str()).unwrap_or("")) {
            let canon = alternate_isbn_form(&norm)
                .filter(|alt| alt.len() == 13)
                .unwrap_or(norm);
            if !distinct_isbns.contains(&canon) {
                distinct_isbns.push(canon);
                if distinct_isbns.len() >= 4 {
                    return true;
                }
            }
        }
    }
    false
}

fn extract_isbn_from_page_blob(blob: &str, allow_multi: bool) -> Option<String> {
    let mut candidates: Vec<(String, bool)> = Vec::new();
    for cap in isbn_re().captures_iter(blob) {
        let Some(m_full) = cap.get(0) else { continue };
        let Some(norm) = normalize_isbn(cap.get(1).map(|m| m.as_str()).unwrap_or("")) else {
            continue;
        };
        let line_slice = {
            let start = blob[..m_full.start()].rfind('\n').map_or(0, |i| i + 1);
            let end = blob[m_full.end()..]
                .find('\n')
                .map_or(blob.len(), |i| m_full.end() + i);
            blob[start..end].to_lowercase()
        };
        let is_set_isbn = line_slice.contains("(set")
            || line_slice.contains("set)")
            || line_slice.contains("collana")
            || line_slice.contains("opera completa")
            || line_slice.contains("complete work");
        candidates.push((norm, is_set_isbn));
    }
    if candidates.is_empty() {
        return None;
    }
    if !allow_multi && candidates.len() >= 6 {
        return None;
    }
    candidates
        .iter()
        .find(|(_, is_set)| !*is_set)
        .or_else(|| candidates.first())
        .map(|(isbn, _)| isbn.clone())
}

fn is_editorial_board_heading(low: &str) -> bool {
    let norm = low.replace(['’', '‘'], "'");
    let t = norm.trim().trim_end_matches(':').trim();
    matches!(
        t,
        "editorial board"
            | "advisory board"
            | "editorial committee"
            | "series editors"
            | "series editor"
            | "managing editor"
            | "managing editors"
            | "founding editors"
            | "founding editor"
            | "honorary editors"
            | "editors-in-chief"
            | "editor-in-chief"
            | "consulting editors"
            | "associate editors"
            | "comitato scientifico"
            | "comitato editoriale"
            | "comitato direttivo"
            | "consiglio scientifico"
            | "direttore della collana"
            | "direttori della collana"
            | "direzione della collana"
            | "direzione scientifica"
            | "piano dell'opera"
            | "presentazione della collana"
            | "nella stessa collana"
            | "volumi pubblicati"
            | "volumi usciti"
            | "elenco dei volumi"
            | "titles in this series"
            | "other titles in this series"
            | "volumes in this series"
            | "published titles"
            | "comité scientifique"
            | "comité de rédaction"
            | "directeur de la collection"
            | "dans la même collection"
            | "wissenschaftlicher beirat"
    ) || norm.starts_with("editorial board:")
        || norm.starts_with("advisory board:")
        || norm.starts_with("series editors:")
        || norm.starts_with("series editor:")
        || norm.starts_with("managing editor:")
        || norm.starts_with("founding editors:")
        || norm.starts_with("editors:")
        || norm.starts_with("collana diretta da")
        || norm.starts_with("direttore della collana:")
        || norm.starts_with("comitato scientifico:")
        || norm.starts_with("collection dirigée par")
        || norm.starts_with("herausgegeben von")
}

fn is_book_series_or_publisher_banner(low: &str) -> bool {
    let t = low
        .trim()
        .replace(['’', '‘'], "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if matches!(
        t.as_str(),
        "publishers' page"
            | "publisher's page"
            | "publishers page"
            | "this page intentionally left blank"
            | "springer"
            | "springer-verlag"
            | "springer italia"
            | "birkhäuser"
            | "birkhauser"
            | "elsevier"
            | "academic press"
            | "world scientific"
            | "cambridge university press"
            | "oxford university press"
            | "princeton university press"
            | "american mathematical society"
            | "european mathematical society"
            | "siam"
            | "dover publications"
            | "de gruyter"
            | "walter de gruyter"
            | "wiley"
            | "john wiley & sons"
            | "north-holland"
            | "crc press"
            | "chapman & hall"
            | "mcgraw-hill"
            | "mir publishers"
            | "edizioni mir"
            | "einaudi"
            | "giulio einaudi editore"
            | "adelphi"
            | "adelphi edizioni"
            | "bollati boringhieri"
            | "boringhieri"
            | "zanichelli"
            | "carocci"
            | "carocci editore"
            | "utet"
            | "mondadori"
            | "feltrinelli"
            | "hoepli"
            | "il mulino"
            | "laterza"
            | "rba"
            | "rba italia"
            | "le scienze"
            | "de agostini"
            | "hachette"
            | "codice edizioni"
            | "raffaello cortina editore"
    ) || is_editorial_board_heading(&t)
    {
        return true;
    }

    if (t.starts_with("collana ")
        || t.starts_with("supplemento a ")
        || t.starts_with("supplemento al ")
        || t.starts_with("edizione speciale per ")
        || t.starts_with("allegato a "))
        && !t.contains(':')
        && t.split_whitespace().count() <= 8
    {
        return true;
    }

    const SERIES_PREFIXES: &[&str] = &[
        "graduate texts in mathematics",
        "undergraduate texts in mathematics",
        "lecture notes in mathematics",
        "lecture notes in computational science and engineering",
        "lecture notes in statistics",
        "lecture notes in physics",
        "grundlehren der mathematischen wissenschaften",
        "ergebnisse der mathematik und ihrer grenzgebiete",
        "applied mathematical sciences",
        "texts in applied mathematics",
        "universitext",
        "unitext",
        "la matematica per il 3+2",
        "convergenze",
        "classics in mathematics",
        "springer monographs in mathematics",
        "modern birkhäuser classics",
        "modern birkhauser classics",
        "progress in mathematics",
        "pure and applied mathematics",
        "encyclopedia of mathematics and its applications",
        "graduate studies in mathematics",
        "student mathematical library",
        "mathematical surveys and monographs",
        "colloquium publications",
        "ams chelsea publishing",
        "carus mathematical monographs",
        "cbms regional conference series",
        "courant lecture notes",
        "cambridge studies in advanced mathematics",
        "cambridge tracts in mathematics",
        "cambridge mathematical library",
        "london mathematical society monographs",
        "london mathematical society lecture note series",
        "london mathematical society student texts",
        "oxford mathematical monographs",
        "oxford lecture series in mathematics and its applications",
        "oxford graduate texts in mathematics",
        "princeton mathematical series",
        "annals of mathematics studies",
        "de gruyter studies in mathematics",
        "north-holland mathematical library",
        "dover books on mathematics",
        "dover books on physics",
        "schaum's outline series",
        "schaum's outlines",
        "astérisque",
        "asterisque",
        "séminaire bourbaki",
        "seminaire bourbaki",
        "mondo matematico",
        "il mondo matematico",
        "grandi idee della scienza",
        "quaderni dell'unione matematica italiana",
        "quaderni umi",
        "biblioteca scientifica",
        "piccola biblioteca einaudi",
        "universale scientifica",
    ];

    for &prefix in SERIES_PREFIXES {
        if let Some(rest) = t.strip_prefix(prefix) {
            let r = rest.trim();
            if r.is_empty() {
                return true;
            }
            // Only treat as a standalone series banner if any trailing tokens are volume/series qualifiers,
            // never if followed by a colon + actual book title.
            if r.starts_with(':') {
                return false;
            }
            let all_qualifiers = r.split_whitespace().all(|w| {
                let cw = w.trim_matches(['.', ',', '-', '#', '(', ')', '[', ']', '/']);
                cw.is_empty()
                    || cw.chars().all(|c| c.is_ascii_digit())
                    || matches!(
                        cw,
                        "vol"
                            | "volume"
                            | "no"
                            | "nr"
                            | "number"
                            | "n"
                            | "band"
                            | "heft"
                            | "tome"
                            | "tomo"
                            | "fasc"
                            | "new"
                            | "series"
                            | "serie"
                            | "second"
                            | "third"
                            | "fourth"
                            | "ns"
                            | "i"
                            | "ii"
                            | "iii"
                            | "iv"
                            | "v"
                            | "vi"
                            | "vii"
                            | "viii"
                            | "ix"
                            | "x"
                    )
            });
            if all_qualifiers {
                return true;
            }
        }
    }
    false
}

fn is_book_half_title_or_boilerplate_page(page_text: &str) -> bool {
    let word_count = page_text.split_whitespace().count();
    if word_count == 0 {
        return true;
    }
    if word_count > 220 {
        return false;
    }
    let low = page_text.to_lowercase();
    if low.contains("abstract") || low.contains("1. introduction") || low.contains("\nintroduction\n") {
        return false;
    }
    let norm_quotes = low.replace(['’', '‘'], "'");
    if norm_quotes.contains("editorial board")
        || norm_quotes.contains("advisory board")
        || norm_quotes.contains("series editors")
        || norm_quotes.contains("managing editor")
        || norm_quotes.contains("founding editors")
        || norm_quotes.contains("publishers' page")
        || norm_quotes.contains("publisher's page")
        || norm_quotes.contains("this page intentionally left blank")
        || norm_quotes.contains("comitato scientifico")
        || norm_quotes.contains("direttore della collana")
        || norm_quotes.contains("collana diretta da")
        || norm_quotes.contains("piano dell'opera")
        || norm_quotes.contains("presentazione della collana")
        || norm_quotes.contains("nella stessa collana")
        || norm_quotes.contains("volumi pubblicati")
        || norm_quotes.contains("titles in this series")
        || norm_quotes.contains("other titles in this series")
        || norm_quotes.contains("volumes in this series")
        || norm_quotes.contains("non vendibile separatamente")
        || norm_quotes.contains("edizione speciale per")
        || (word_count <= 90 && is_copyright_or_colophon_blob(page_text) && !low.contains("university") && !contains_title_topic_word(page_text))
    {
        return true;
    }
    let non_empty: Vec<&str> = page_text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    non_empty.len() <= 6
        && non_empty.iter().all(|l| {
            l.len() < 4
                || l.chars().all(|c| c.is_ascii_digit() || c.is_whitespace())
                || is_book_series_or_publisher_banner(&l.to_lowercase())
        })
}

fn extract_cover_page_authors(page_text: &str) -> Vec<String> {
    for line in page_text.lines() {
        let trimmed = line.trim();
        let low = trimmed.to_ascii_lowercase();
        for prefix in ["author(s):", "authors:", "auteur(s):", "auteurs:"] {
            if low.starts_with(prefix) {
                let rest = trimmed[prefix.len()..].trim();
                if !rest.is_empty() {
                    return split_author_list(rest);
                }
            }
        }
    }
    Vec::new()
}

fn extract_pre_colon_main_title(title: &str) -> Option<String> {
    for sep in [": ", " — ", " – "] {
        if let Some((left, right)) = title.split_once(sep) {
            let l = left.trim();
            let r = right.trim();
            if l.split_whitespace().count() >= 4 && l.len() >= 22 && r.len() >= 6 {
                return Some(l.to_string());
            }
        }
    }
    None
}

/// Extracts clean alphabetic prose words (`>= 3` letters) from titles containing mangled PDF math
/// formulas (`L^2`, `C^{1,\alpha}`, `p(x)-Laplacian`, `\mathbb{R}^n`), returning `Some(...)` only
/// when math noise was actually removed and at least 3 substantial words (`>= 4` letters) remain.
pub(crate) fn extract_prose_core_title(title: &str) -> Option<String> {
    let cleaned = clean_ot1_and_ligatures_preserve_case(title);
    let raw_tokens: Vec<&str> = cleaned.split_whitespace().collect();
    if raw_tokens.len() < 4 {
        return None;
    }
    let mut kept: Vec<String> = Vec::new();
    let mut dropped_noise = false;

    for tok in &raw_tokens {
        let trimmed = tok.trim_matches(|c: char| {
            matches!(c, ',' | '.' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'')
        });
        if trimmed.is_empty() {
            dropped_noise = true;
            continue;
        }
        // Handle hyphenated math prefixes like "p(x)-Laplacian" or "L2-estimates" -> keep "Laplacian" / "estimates"
        if trimmed.contains('-') {
            let parts: Vec<&str> = trimmed.split('-').collect();
            let all_clean_alpha = parts.iter().all(|p| {
                p.len() >= 2 && p.chars().all(|c| c.is_alphabetic() && !is_greek_math_char(c))
            });
            if all_clean_alpha {
                kept.push(trimmed.to_string());
                continue;
            }
            let good_parts: Vec<&str> = parts
                .into_iter()
                .filter(|p| {
                    p.len() >= 4
                        && p.chars()
                            .all(|c| c.is_alphabetic() && !is_greek_math_char(c))
                })
                .collect();
            dropped_noise = true;
            for gp in good_parts {
                kept.push(gp.to_string());
            }
            continue;
        }

        let is_pure_prose = trimmed
            .chars()
            .all(|c| c.is_alphabetic() && !is_greek_math_char(c));
        let low = trimmed.to_ascii_lowercase();
        let is_short_connector = matches!(
            low.as_str(),
            "on" | "in" | "of" | "to" | "by" | "at" | "as" | "or" | "an"
        );
        if is_pure_prose && (trimmed.chars().count() >= 3 || is_short_connector) {
            kept.push(trimmed.to_string());
        } else {
            dropped_noise = true;
        }
    }

    let substantial = kept.iter().filter(|w| w.chars().count() >= 4).count();
    if dropped_noise && substantial >= 3 {
        Some(kept.join(" "))
    } else {
        None
    }
}

fn is_greek_math_char(c: char) -> bool {
    ('\u{0370}'..='\u{03FF}').contains(&c) || ('\u{2200}'..='\u{22FF}').contains(&c)
}

fn choose_best_title(embedded: Option<String>, page_title: Option<String>) -> Option<String> {
    match (embedded, page_title) {
        (Some(emb), Some(pg)) => {
            let emb_words = emb.split_whitespace().count();
            let pg_words = pg.split_whitespace().count();
            let emb_low = normalize_alpha_space(&emb);
            let pg_low = normalize_alpha_space(&pg);
            // Prefer layout/page title if embedded PDF /Title was truncated at line 1 or is a single short word
            if (!emb_low.is_empty()
                && pg_low.starts_with(&emb_low)
                && pg.len() >= emb.len() + 8)
                || (emb_words <= 1 && emb.len() < 20 && pg_words >= 3)
            {
                Some(pg)
            } else {
                Some(emb)
            }
        }
        (Some(emb), None) => Some(emb),
        (None, Some(pg)) => Some(pg),
        (None, None) => None,
    }
}

fn strip_references_and_citations(page_text: &str) -> String {
    static CIT_LINE_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let cit_re = CIT_LINE_RE.get_or_init(|| {
        Regex::new(r"^\s*(?:\[\d{1,3}\]|\(\d{1,3}\)|\d{1,3}\.\s+[A-Z])").unwrap()
    });

    let mut out_lines = Vec::new();
    let mut in_references = false;
    for line in page_text.lines() {
        let trimmed = line.trim();
        let low = trimmed.to_ascii_lowercase();
        let heading = low.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ');
        if matches!(
            heading,
            "references"
                | "bibliography"
                | "bibliographie"
                | "literatur"
                | "literaturverzeichnis"
                | "riferimenti"
                | "riferimenti bibliografici"
                | "bibliografia"
                | "works cited"
        ) {
            in_references = true;
            continue;
        }
        if in_references {
            // On split-issue journal pages where the previous article's References end near the top of page 1
            // and the new article starts below, resume once an unmistakable new-article boundary appears.
            if is_body_boundary_line(&low) || low.starts_with("received ") {
                in_references = false;
            } else {
                continue;
            }
        }
        if cit_re.is_match(trimmed) {
            continue;
        }
        out_lines.push(line);
    }
    out_lines.join("\n")
}

fn text_confirms_embedded_title(embedded: &str, first_pages: &[&str]) -> bool {
    let front_raw: String = first_pages.iter().take(3).copied().collect::<Vec<_>>().join(" ");
    let front_alpha = front_raw.chars().filter(|c| c.is_alphabetic()).count();
    if front_alpha < 60 {
        return true;
    }
    let front_norm = normalize_math_text(&front_raw);
    let emb_norm = normalize_math_text(embedded);
    let sig: Vec<&str> = emb_norm
        .split_whitespace()
        .filter(|w| w.len() >= 4)
        .collect();
    if sig.is_empty() {
        return !emb_norm.is_empty() && front_norm.contains(&emb_norm);
    }
    let hits = sig.iter().filter(|w| front_norm.contains(**w)).count();
    hits * 2 >= sig.len()
}

fn text_confirms_embedded_author(author_raw: &str, first_pages: &[&str]) -> bool {
    let front_raw: String = first_pages.iter().take(3).copied().collect::<Vec<_>>().join(" ");
    let front_alpha = front_raw.chars().filter(|c| c.is_alphabetic()).count();
    if front_alpha < 60 {
        return true;
    }
    let front_norm = normalize_math_text(&front_raw);
    let parts = split_author_list(author_raw);
    if parts.is_empty() {
        return false;
    }
    parts.iter().any(|p| {
        clean_author_surname(p)
            .map(|s| normalize_math_text(&s))
            .filter(|s| s.len() >= 3)
            .is_some_and(|sn| front_norm.contains(&sn))
    })
}

fn normalize_alpha_space(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn push_unique_author(acc: &mut Vec<String>, candidate: String) {
    let trimmed = candidate.trim();
    if trimmed.len() < 2 {
        return;
    }
    let cand_surname = clean_author_surname(trimmed).map(|s| s.to_ascii_lowercase());
    if let Some(ref cs) = cand_surname {
        if acc.iter().any(|existing| {
            clean_author_surname(existing)
                .map(|s| s.to_ascii_lowercase() == *cs)
                .unwrap_or(false)
        }) {
            return;
        }
    }
    acc.push(trimmed.to_string());
}

fn infer_year_from_arxiv_id(arxiv_id: &str) -> Option<i64> {
    // New-style YYMM.NNNNN (started 0704)
    let bytes = arxiv_id.as_bytes();
    if bytes.len() >= 9 && bytes[4] == b'.' && bytes[..4].iter().all(|b| b.is_ascii_digit()) {
        let yy: i64 = arxiv_id[..2].parse().ok()?;
        let mm: i64 = arxiv_id[2..4].parse().ok()?;
        if (7..=35).contains(&yy) && (1..=12).contains(&mm) {
            return Some(2000 + yy);
        }
    }
    // Old-style arch-ive/YYMMNNN (91..99 -> 1991..1999, 00..07 -> 2000..2007)
    if let Some((_, num)) = arxiv_id.split_once('/') {
        if num.len() >= 7 && num.bytes().take(4).all(|b| b.is_ascii_digit()) {
            let yy: i64 = num[..2].parse().ok()?;
            let mm: i64 = num[2..4].parse().ok()?;
            if (1..=12).contains(&mm) {
                if (91..=99).contains(&yy) {
                    return Some(1900 + yy);
                } else if (0..=7).contains(&yy) {
                    return Some(2000 + yy);
                }
            }
        }
    }
    None
}

fn extract_clean_year(text: &str) -> Option<i64> {
    let cleaned = msc_year_noise_re().replace_all(text, " ");
    for cap in year_re().captures_iter(&cleaned) {
        if let Some(m) = cap.get(1) {
            if let Ok(yr) = m.as_str().parse::<i64>() {
                if (1900..=2028).contains(&yr) {
                    // Avoid matching 4-digit page ranges like "1912-1945" or "1912–1945" where the second number > first + 2
                    let after = &cleaned[m.end()..];
                    let trimmed_after = after.trim_start_matches(' ');
                    if let Some(rest) = trimmed_after
                        .strip_prefix("--")
                        .or_else(|| trimmed_after.strip_prefix('-'))
                        .or_else(|| trimmed_after.strip_prefix('–'))
                    {
                        let digits: String = rest
                            .trim_start_matches(' ')
                            .chars()
                            .take_while(|c| c.is_ascii_digit())
                            .collect();
                        if digits.len() == 4 {
                            if let Ok(second) = digits.parse::<i64>() {
                                if second > yr + 3 && second < yr + 120 {
                                    continue;
                                }
                            }
                        }
                    }
                    return Some(yr);
                }
            }
        }
    }
    None
}

pub(crate) fn strip_inline_markup_tags(raw: &str) -> String {
    static TAG_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let re = TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").unwrap());
    let replaced = re.replace_all(raw, " ");
    let joined = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    clean_ot1_and_ligatures_preserve_case(&joined)
}

pub(crate) fn looks_like_garbage_title(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.len() < 4 {
        return true;
    }
    let low = trimmed.to_ascii_lowercase();
    for ext in [
        ".pdf", ".djvu", ".dvi", ".tex", ".doc", ".docx", ".ps", ".eps", ".indd", ".qxd", ".pmd",
        ".txt",
    ] {
        if low.ends_with(ext) {
            return true;
        }
    }
    if low.starts_with("microsoft word")
        || low.starts_with("acrobat distiller")
        || low.starts_with("pii:")
        || low.starts_with("doi:")
        || low.starts_with("arxiv:")
        || low.starts_with("isbn")
        || low.starts_with("issn")
        || low.starts_with("finito di stampare")
        || low.starts_with("tutti i diritti riservati")
        || low.starts_with("http://")
        || low.starts_with("https://")
        || matches!(
            low.as_str(),
            "untitled"
                | "document"
                | "paper"
                | "preprint"
                | "manuscript"
                | "article"
                | "main"
                | "fulltext"
                | "scan"
                | "latex"
                | "pdftex"
                | "dvips"
        )
        || is_running_journal_header(&low)
        || is_book_series_or_publisher_banner(&low)
    {
        return true;
    }
    // Single-token machine IDs or author+year stems like "brezzi1974" or "S002199919795682X"
    if !trimmed.contains(' ') {
        let alpha_count = trimmed.chars().filter(|c| c.is_alphabetic()).count();
        let digit_count = trimmed.chars().filter(|c| c.is_ascii_digit()).count();
        if alpha_count < 5 || (digit_count >= 3 && trimmed.len() <= 28) {
            return true;
        }
    }
    false
}

fn looks_like_garbage_author(s: &str) -> bool {
    let low = s.trim().to_ascii_lowercase();
    low.is_empty()
        || matches!(
            low.as_str(),
            "unknown"
                | "author"
                | "authors"
                | "editor"
                | "editors"
                | "board"
                | "user"
                | "admin"
                | "administrator"
                | "ieee"
                | "elsevier"
                | "springer"
                | "springer-verlag"
                | "birkhäuser"
                | "birkhauser"
                | "wiley"
                | "academic press"
                | "world scientific"
                | "cambridge university press"
                | "oxford university press"
                | "princeton university press"
                | "american mathematical society"
                | "dover publications"
        )
        || is_editorial_board_heading(&low)
        || is_book_series_or_publisher_banner(&low)
        || low.contains('@')
        || low.starts_with("http")
}

fn is_running_journal_header(low: &str) -> bool {
    (low.contains("submitted")
        && (low.contains("revised") || low.contains("published") || low.contains("accepted")))
        || low.starts_with("journal of ")
        || low.starts_with("transactions of ")
        || low.starts_with("proceedings of ")
        || low.starts_with("annals of ")
        || low.starts_with("communications in ")
        || low.starts_with("communications on ")
        || low.starts_with("bulletin of ")
        || low.starts_with("siam journal ")
        || low.starts_with("siam j.")
        || low.starts_with("inventiones mathematicae")
        || low.starts_with("mathematische annalen")
        || low.starts_with("numerische mathematik")
        || low.starts_with("archive for rational")
        || low.starts_with("comptes rendus")
        || low.contains("issn")
        || low.contains("downloaded from")
        || low.contains("copyright")
        || low.contains("all rights reserved")
        || low.contains("elsevier")
        || low.contains("springer-verlag")
        || low.contains("science direct")
        || low.contains("sciencedirect")
        || (low.starts_with("vol.") && (low.contains("no.") || low.contains("pp.")))
}

fn is_body_boundary_line(low: &str) -> bool {
    let t = low.trim();
    t == "abstract"
        || t.starts_with("abstract.")
        || t.starts_with("abstract:")
        || t.starts_with("abstract—")
        || t.starts_with("abstract -")
        || t.starts_with("abstract ")
            && t.len() > 40 // Inline "Abstract We study..." paragraph
        || t == "résumé"
        || t.starts_with("résumé.")
        || t.starts_with("résumé:")
        || t == "summary"
        || t.starts_with("summary.")
        || t == "zusammenfassung"
        || t == "riassunto"
        || t == "sunto"
        || t == "introduction"
        || t.starts_with("1. introduction")
        || t.starts_with("1 introduction")
        || t.starts_with("i. introduction")
        || t == "contents"
        || t == "table of contents"
}

fn is_non_title_seed_line(text: &str) -> bool {
    let trimmed = text.trim();
    let alpha_count = trimmed.chars().filter(|c| c.is_alphabetic()).count();
    if alpha_count < 4 || trimmed.len() < 5 || trimmed.len() > 220 {
        return true;
    }
    let low = trimmed.to_ascii_lowercase();
    if looks_like_garbage_title(trimmed) || is_body_boundary_line(&low) {
        return true;
    }
    let heading = low.trim_start_matches(|c: char| {
        c.is_ascii_digit() || c == '.' || c == '§' || c == ' '
    });
    if matches!(
        heading,
        "references"
            | "bibliography"
            | "bibliographie"
            | "literatur"
            | "literaturverzeichnis"
            | "works cited"
            | "acknowledgments"
            | "acknowledgements"
            | "preliminaries"
            | "notation"
    ) {
        return true;
    }
    matches!(
        low.as_str(),
        "original paper"
            | "original article"
            | "research article"
            | "research paper"
            | "review article"
            | "regular paper"
            | "regular article"
            | "short communication"
            | "letter"
            | "letters"
    ) || low.starts_with("communicated by ")
        || low.starts_with("dedicated to ")
        || low.starts_with("in memory of ")
        || low.starts_with("received ")
        || low.starts_with("accepted ")
        || low.starts_with("published online")
        || low.starts_with("available online")
        || low.starts_with("preprint submitted")
        || low.contains("subject classification")
        || low.starts_with("key words")
        || low.starts_with("keywords")
        || looks_like_affiliation_line(trimmed)
}

fn looks_like_affiliation_line(text: &str) -> bool {
    let low = text.trim().to_ascii_lowercase();
    if low.contains('@') || low.contains("http://") || low.contains("https://") {
        return true;
    }
    for kw in [
        "department of",
        "dipartimento di",
        "département de",
        "institut für",
        "institute of",
        "institute for",
        "courant institute",
        "laboratory of",
        "laboratoire",
        "faculty of",
        "school of ",
        "academy of sciences",
        "università ",
        "université ",
        "universität ",
        "universidad ",
        "technical report",
        "rapport de recherche",
        "research report",
        "working paper no",
        "preprint no",
    ] {
        if low.contains(kw) {
            return true;
        }
    }
    if low.contains("university") {
        // E.g. "Stanford University, Stanford, CA 94305" or "University of Chicago"
        let words = low.split_whitespace().count();
        if words <= 10
            || low.contains(',')
            || low.contains("usa")
            || low.contains("u.s.a")
            || low.contains("uk")
        {
            return true;
        }
    }
    false
}

const TITLE_TOPIC_WORDS: &[&str] = &[
    "approximation",
    "approximations",
    "analysis",
    "algorithm",
    "algorithms",
    "algebra",
    "algebras",
    "application",
    "applications",
    "asymptotic",
    "approach",
    "bound",
    "bounds",
    "boundary",
    "calculus",
    "characterization",
    "classification",
    "cohomology",
    "convergence",
    "convex",
    "differential",
    "diffusion",
    "dimension",
    "dirichlet",
    "discrete",
    "distribution",
    "distributions",
    "dynamics",
    "dynamical",
    "eigenvalue",
    "eigenvalues",
    "element",
    "elements",
    "elliptic",
    "energy",
    "entropy",
    "equation",
    "equations",
    "error",
    "estimate",
    "estimates",
    "estimation",
    "evolution",
    "existence",
    "expansion",
    "finite",
    "flow",
    "flows",
    "formula",
    "formulation",
    "fourier",
    "fractional",
    "free",
    "function",
    "functional",
    "functions",
    "galerkin",
    "gaussian",
    "generalized",
    "geometric",
    "geometry",
    "gradient",
    "graph",
    "graphs",
    "group",
    "groups",
    "harmonic",
    "heat",
    "hilbert",
    "homogenization",
    "hyperbolic",
    "inequalities",
    "inequality",
    "infinite",
    "integral",
    "integrals",
    "interpolation",
    "invariant",
    "invariants",
    "inverse",
    "iterative",
    "kernel",
    "kernels",
    "lagrange",
    "laplacian",
    "learning",
    "lecture",
    "lectures",
    "lemma",
    "limit",
    "limits",
    "linear",
    "local",
    "manifold",
    "manifolds",
    "mapping",
    "mappings",
    "markov",
    "matrices",
    "matrix",
    "measure",
    "measures",
    "mechanics",
    "method",
    "methods",
    "metric",
    "minimal",
    "minimization",
    "mixed",
    "model",
    "models",
    "moduli",
    "monotone",
    "multigrid",
    "multiscale",
    "navier",
    "network",
    "networks",
    "neural",
    "nonlinear",
    "nonlocal",
    "norm",
    "norms",
    "note",
    "notes",
    "numerical",
    "operator",
    "operators",
    "optimal",
    "optimization",
    "order",
    "ordinary",
    "orthogonal",
    "parabolic",
    "parameter",
    "partial",
    "particle",
    "periodic",
    "perturbation",
    "phase",
    "piecewise",
    "point",
    "points",
    "poisson",
    "polynomial",
    "polynomials",
    "potential",
    "principle",
    "principles",
    "probabilistic",
    "probabilities",
    "probability",
    "problem",
    "problems",
    "process",
    "processes",
    "projection",
    "properties",
    "property",
    "quantum",
    "random",
    "rate",
    "rates",
    "regularity",
    "regularization",
    "remarks",
    "representation",
    "representations",
    "riemannian",
    "saddle",
    "sampling",
    "scalar",
    "scheme",
    "schemes",
    "semigroup",
    "semilinear",
    "series",
    "singular",
    "smooth",
    "sobolev",
    "solution",
    "solutions",
    "space",
    "spaces",
    "sparse",
    "spectral",
    "spectrum",
    "sphere",
    "spherical",
    "stability",
    "statistical",
    "statistics",
    "stochastic",
    "stokes",
    "structure",
    "structures",
    "subspace",
    "surface",
    "surfaces",
    "survey",
    "symmetric",
    "symmetry",
    "system",
    "systems",
    "tensor",
    "test",
    "testing",
    "theorem",
    "theorems",
    "theoretical",
    "theory",
    "thermodynamics",
    "topological",
    "topology",
    "transform",
    "transport",
    "turbulence",
    "uniqueness",
    "variational",
    "vector",
    "viscosity",
    "wave",
    "waves",
    "weak",
];

fn contains_title_topic_word(text: &str) -> bool {
    for raw in text.split(|c: char| !c.is_alphabetic()) {
        if raw.len() >= 4 {
            let low = raw.to_ascii_lowercase();
            if TITLE_TOPIC_WORDS.binary_search(&low.as_str()).is_ok()
                || TITLE_TOPIC_WORDS.contains(&low.as_str())
            {
                return true;
            }
        }
    }
    false
}

fn ends_with_title_connector(text: &str) -> bool {
    let trimmed = text.trim_end();
    if trimmed.ends_with([':', '-', '–', '—', ',']) {
        return true;
    }
    let last_word = trimmed
        .rsplit(|c: char| c.is_whitespace())
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        last_word.as_str(),
        "of" | "in"
            | "on"
            | "for"
            | "with"
            | "by"
            | "to"
            | "from"
            | "and"
            | "or"
            | "the"
            | "a"
            | "an"
            | "over"
            | "under"
            | "via"
            | "between"
            | "into"
            | "through"
            | "without"
            | "near"
            | "at"
            | "as"
            | "upon"
    )
}

fn starts_with_title_connector(text: &str) -> bool {
    let trimmed = text.trim_start();
    let first_word = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    // Note: "by" is intentionally excluded here because "By Author Name" starts an author line!
    matches!(
        first_word.as_str(),
        "of" | "in"
            | "on"
            | "for"
            | "with"
            | "to"
            | "from"
            | "or"
            | "over"
            | "under"
            | "via"
            | "between"
            | "into"
            | "through"
            | "without"
            | "associated"
            | "related"
            | "using"
    )
}

fn is_all_caps_text(text: &str) -> bool {
    let mut upper = 0usize;
    let mut lower = 0usize;
    for ch in text.chars() {
        if ch.is_uppercase() {
            upper += 1;
        } else if ch.is_lowercase() {
            lower += 1;
        }
    }
    upper >= 4 && lower * 5 <= upper
}

fn strip_author_markers(raw: &str) -> String {
    let mut s = raw.trim();
    for prefix in ["by ", "By ", "BY "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim();
        }
    }
    // Strip footnote symbols (*, †, ‡, §, ¶, superscript digits) and trailing affiliation digits
    let cleaned: String = s
        .chars()
        .filter(|&c| {
            !matches!(
                c,
                '*' | '†' | '‡' | '§' | '¶' | '¹' | '²' | '³' | '⁴' | '⁵' | '⁶' | '⁷' | '⁸' | '⁹' | '⁰'
            )
        })
        .collect();
    static FOOTNOTE_DIGIT_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let re = FOOTNOTE_DIGIT_RE
        .get_or_init(|| Regex::new(r"(?:\s*\d+(?:\s*,\s*\d+)*|\b\d+\b)").unwrap());
    let without_digits = re.replace_all(&cleaned, " ");
    without_digits.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Checks whether a single header line looks like a person-name list (`"David Mumford and Jayant Shah"`,
/// `"F. Brezzi, M. Fortin"`, `"BY JOHN FORBES NASH, JR."`).
fn looks_like_author_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 4 || trimmed.len() > 140 {
        return false;
    }
    let low = trimmed.to_ascii_lowercase();
    if is_body_boundary_line(&low)
        || is_running_journal_header(&low)
        || is_editorial_board_heading(&low)
        || is_book_series_or_publisher_banner(&low)
        || looks_like_garbage_author(&low)
        || looks_like_affiliation_line(trimmed)
        || contains_title_topic_word(trimmed)
    {
        return false;
    }
    let starts_by = low.starts_with("by ");
    let cleaned = strip_author_markers(trimmed);
    if cleaned.is_empty() {
        return false;
    }

    let chunks = split_author_list(&cleaned);
    if chunks.is_empty() || chunks.len() > 10 {
        return false;
    }

    let all_caps = is_all_caps_text(&cleaned);
    let mut valid_names = 0usize;
    for chunk in &chunks {
        let words: Vec<&str> = chunk.split_whitespace().collect();
        if words.is_empty() || words.len() > 5 {
            return false;
        }
        // Single-word author chunks are invalid unless preceded by "By "
        if words.len() == 1 && !starts_by {
            return false;
        }
        let words_ok = words.iter().all(|w| {
            let clean_w = w.trim_matches(['.', '-', '\'']);
            if clean_w.is_empty() {
                return false;
            }
            if matches!(
                clean_w.to_ascii_lowercase().as_str(),
                "de" | "del" | "della" | "di" | "du" | "van" | "von" | "der" | "den" | "la" | "le" | "jr" | "sr" | "ii" | "iii"
            ) {
                return true;
            }
            let first_upper = clean_w.chars().next().is_some_and(char::is_uppercase);
            let alpha_ok = clean_w
                .chars()
                .all(|c| c.is_alphabetic() || c == '-' || c == '\'' || c == '.');
            (first_upper || all_caps) && alpha_ok
        });
        if !words_ok {
            return false;
        }
        valid_names += 1;
    }
    valid_names >= 1
}

/// Splits an author line into clean individual author display strings, handling both
/// `"Given1 Family1, Given2 Family2"` and single `"Family, Given"` formats.
fn split_author_list(raw: &str) -> Vec<String> {
    let cleaned = strip_author_markers(raw);
    if cleaned.is_empty() {
        return Vec::new();
    }
    let normalized = cleaned
        .replace(" AND ", ";")
        .replace(" and ", ";")
        .replace(" & ", ";")
        .replace('&', ";");

    let mut out = Vec::new();
    for semi_part in normalized.split(';') {
        let sp = semi_part.trim().trim_matches(',');
        if sp.is_empty() {
            continue;
        }
        let comma_parts: Vec<&str> = sp.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        if comma_parts.len() == 2 {
            let p0_words = comma_parts[0].split_whitespace().count();
            let p1_words = comma_parts[1].split_whitespace().count();
            let p1_looks_given_only = p1_words == 1
                && (comma_parts[1].ends_with('.') || comma_parts[1].chars().count() <= 3);
            if p0_words == 1 && (p1_looks_given_only || p1_words == 1) {
                // "Brezzi, F." or "Brezzi, Franco"
                out.push(format!("{} {}", comma_parts[1], comma_parts[0]));
                continue;
            }
        }
        for cp in comma_parts {
            let cp_clean = cp.trim();
            if matches!(cp_clean.to_ascii_lowercase().as_str(), "jr." | "jr" | "sr." | "sr" | "iii" | "ii") {
                if let Some(last) = out.last_mut() {
                    last.push(' ');
                    last.push_str(cp_clean);
                }
                continue;
            }
            if cp_clean.chars().filter(|c| c.is_alphabetic()).count() >= 2 {
                out.push(normalize_all_caps_name(cp_clean));
            }
        }
    }
    out
}

fn normalize_all_caps_name(name: &str) -> String {
    if !is_all_caps_text(name) {
        return name.to_string();
    }
    name.split_whitespace()
        .map(|w| {
            if w.len() <= 2 && w.ends_with('.') {
                w.to_string()
            } else {
                let mut chars = w.chars();
                match chars.next() {
                    Some(first) => {
                        format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase())
                    }
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone)]
struct LayoutLine {
    text: String,
    y: f32,
    bottom: f32,
    h: f32,
}

fn build_layout_lines(page1: &ExtractedPage) -> Vec<LayoutLine> {
    #[derive(Clone)]
    struct RawSpan<'a> {
        text: &'a str,
        b: NormalizedRect,
        cy: f32,
    }

    impl LayoutSpanView for RawSpan<'_> {
        fn span_text(&self) -> &str {
            self.text
        }
        fn span_rect(&self) -> NormalizedRect {
            self.b
        }
    }

    let mut spans: Vec<RawSpan<'_>> = page1
        .spans
        .iter()
        .filter_map(|s| {
            let t = s.text.trim();
            if t.is_empty() {
                return None;
            }
            let b = s.bbox;
            // Filter extreme left/right vertical margin stamps (e.g. rotated arXiv / journal watermarks)
            if (b.x + b.w < 0.075)
                || (b.x > 0.925)
                || (b.w < 0.035 && b.h > b.w * 2.2 && t.len() > 3)
                || b.y > 0.80
            {
                return None;
            }
            Some(RawSpan {
                text: t,
                b,
                cy: b.y + 0.5 * b.h,
            })
        })
        .collect();

    spans.sort_by(|a, b| {
        a.cy.partial_cmp(&b.cy)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.b.x.partial_cmp(&b.b.x).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut groups: Vec<Vec<RawSpan<'_>>> = Vec::new();
    for sp in spans {
        let mut attached = false;
        if let Some(last_group) = groups.last_mut() {
            let grp_h = last_group
                .iter()
                .map(|g| g.b.h)
                .fold(0.008_f32, f32::max);
            let grp_cy = last_group.iter().map(|g| g.cy).sum::<f32>() / (last_group.len() as f32);
            if (sp.cy - grp_cy).abs() <= 0.65 * grp_h.max(sp.b.h) {
                last_group.push(sp.clone());
                attached = true;
            }
        }
        if !attached {
            groups.push(vec![sp]);
        }
    }

    let mut lines = Vec::with_capacity(groups.len());
    for mut grp in groups {
        grp.sort_by(|a, b| a.b.x.partial_cmp(&b.b.x).unwrap_or(std::cmp::Ordering::Equal));
        let raw_joined = join_group_spans_handling_letter_spacing(&grp);
        let text = collapse_letter_spaced_line(&clean_ot1_and_ligatures_preserve_case(&raw_joined))
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }
        let y = grp.iter().map(|g| g.b.y).fold(1.0_f32, f32::min);
        let bottom = grp
            .iter()
            .map(|g| g.b.y + g.b.h)
            .fold(0.0_f32, f32::max);
        // Dominant font height from substantial spans (ignoring tiny superscripts/subscripts)
        let dominant_h = grp
            .iter()
            .filter(|g| g.text.chars().filter(|c| c.is_alphabetic()).count() >= 3)
            .map(|g| g.b.h)
            .fold(0.0_f32, f32::max);
        let h = if dominant_h > 0.0 {
            dominant_h
        } else {
            grp.iter().map(|g| g.b.h).fold(0.01_f32, f32::max)
        };
        lines.push(LayoutLine {
            text,
            y,
            bottom,
            h,
        });
    }
    lines
}

fn join_group_spans_handling_letter_spacing<T>(grp: &[T]) -> String
where
    T: LayoutSpanView,
{
    if grp.len() >= 5 {
        let single_alpha = grp
            .iter()
            .filter(|g| {
                let t = g.span_text();
                t.chars().count() == 1 && t.chars().all(char::is_alphabetic)
            })
            .count();
        if single_alpha * 100 >= grp.len() * 65 {
            let mut gaps: Vec<f32> = Vec::new();
            for pair in grp.windows(2) {
                let g = (pair[1].span_rect().x - (pair[0].span_rect().x + pair[0].span_rect().w))
                    .max(0.0);
                gaps.push(g);
            }
            let mut sorted_gaps = gaps.clone();
            sorted_gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let med_gap = sorted_gaps[sorted_gaps.len() / 2].max(0.0015);
            let mut out = String::new();
            for (i, sp) in grp.iter().enumerate() {
                if i > 0 {
                    let gap = (sp.span_rect().x - (grp[i - 1].span_rect().x + grp[i - 1].span_rect().w))
                        .max(0.0);
                    if gap > 1.75 * med_gap {
                        out.push(' ');
                    }
                }
                out.push_str(sp.span_text());
            }
            return out;
        }
    }
    grp.iter()
        .map(|g| g.span_text())
        .collect::<Vec<_>>()
        .join(" ")
}

trait LayoutSpanView {
    fn span_text(&self) -> &str;
    fn span_rect(&self) -> NormalizedRect;
}

/// Collapses letter-spaced plain-text headings such as `"O N   S A D D L E   P O I N T S"`
/// or `"F R A N C O   B R E Z Z I"` while leaving normal prose untouched.
fn collapse_letter_spaced_line(raw: &str) -> String {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    if tokens.len() < 4 {
        return raw.to_string();
    }
    let single_alpha = tokens
        .iter()
        .filter(|t| t.chars().count() == 1 && t.chars().all(char::is_alphabetic))
        .count();
    if raw.contains("  ") && tokens.len() >= 5 && single_alpha * 100 >= tokens.len() * 65 {
        return raw
            .split("  ")
            .map(str::trim)
            .filter(|seg| !seg.is_empty())
            .map(|seg| {
                let sub: Vec<&str> = seg.split_whitespace().collect();
                if sub.iter().all(|t| t.chars().count() == 1) {
                    sub.join("")
                } else {
                    sub.join(" ")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    if (4..=9).contains(&tokens.len())
        && tokens
            .iter()
            .all(|t| t.chars().count() == 1 && t.chars().all(char::is_alphabetic))
    {
        return tokens.join("");
    }
    raw.to_string()
}

fn join_title_lines(lines: &[&str]) -> String {
    let mut acc = String::new();
    for line in lines {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if acc.is_empty() {
            acc.push_str(t);
        } else if acc.ends_with('-')
            && acc.len() >= 2
            && acc
                .chars()
                .rev()
                .nth(1)
                .is_some_and(char::is_alphabetic)
            && t.chars().next().is_some_and(char::is_lowercase)
        {
            acc.pop();
            acc.push_str(t);
        } else {
            acc.push(' ');
            acc.push_str(t);
        }
    }
    acc.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_references_heading_line(low: &str) -> bool {
    let heading = low
        .trim()
        .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '§' || c == ' ');
    matches!(
        heading,
        "references"
            | "bibliography"
            | "bibliographie"
            | "literatur"
            | "literaturverzeichnis"
            | "works cited"
    )
}

fn looks_like_citation_item_line(trimmed: &str) -> bool {
    static CIT_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let re = CIT_RE.get_or_init(|| {
        Regex::new(r"^\s*(?:\[\d{1,3}\]|\(\d{1,3}\)|\d{1,3}\.\s+[A-Z])").unwrap()
    });
    re.is_match(trimmed)
}

fn extract_layout_title_and_authors(page1: &ExtractedPage) -> (Option<String>, Vec<String>) {
    let lines = build_layout_lines(page1);
    if lines.is_empty() {
        return (None, Vec::new());
    }

    // Detect split-issue archival pages where the previous article's References finish in the upper half
    // of page 1 and the target article begins underneath.
    let mut min_header_y = 0.0_f32;
    if let Some(ref_idx) = lines
        .iter()
        .position(|ln| ln.y < 0.52 && is_references_heading_line(&ln.text.to_ascii_lowercase()))
    {
        let has_new_article_below = lines.iter().skip(ref_idx + 1).any(|ln| {
            is_body_boundary_line(&ln.text.to_ascii_lowercase())
                || (ln.h >= lines[ref_idx].h * 1.20 && !looks_like_citation_item_line(&ln.text))
        });
        if has_new_article_below {
            min_header_y = lines[ref_idx].bottom;
            for ln in lines.iter().skip(ref_idx + 1) {
                if looks_like_citation_item_line(&ln.text)
                    || (ln.h <= lines[ref_idx].h * 1.05
                        && !is_body_boundary_line(&ln.text.to_ascii_lowercase())
                        && ln.y - min_header_y < 0.045)
                {
                    min_header_y = ln.bottom;
                } else {
                    break;
                }
            }
        }
    }

    // Find top of Abstract / Introduction below min_header_y so we only search the true front-matter zone
    let mut header_limit_y = if min_header_y > 0.0 { 0.76_f32 } else { 0.66_f32 };
    for ln in &lines {
        if ln.y >= (min_header_y + 0.04).max(0.10)
            && is_body_boundary_line(&ln.text.to_ascii_lowercase())
        {
            header_limit_y = ln.y;
            break;
        }
    }

    let header_indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, ln)| ln.y >= min_header_y && ln.y < header_limit_y)
        .map(|(i, _)| i)
        .collect();

    if header_indices.is_empty() {
        return (None, Vec::new());
    }

    let max_title_h = header_indices
        .iter()
        .map(|&i| &lines[i])
        .filter(|ln| !is_non_title_seed_line(&ln.text))
        .map(|ln| ln.h)
        .fold(0.0_f32, f32::max);

    if max_title_h <= 0.0 {
        return (None, Vec::new());
    }

    // Pick the topmost valid seed line whose font height is within 10% of max_title_h
    let Some(&seed_idx) = header_indices.iter().find(|&&i| {
        let ln = &lines[i];
        !is_non_title_seed_line(&ln.text) && ln.h >= 0.90 * max_title_h
    }) else {
        return (None, Vec::new());
    };

    let seed_h = lines[seed_idx].h;
    let mut start_idx = seed_idx;
    let mut end_idx = seed_idx;

    // Expand upward if an immediately preceding line has matching font height and tight spacing
    while start_idx > 0 {
        let prev_idx = start_idx - 1;
        let prev = &lines[prev_idx];
        let curr = &lines[start_idx];
        if prev.y < min_header_y
            || prev.y >= header_limit_y
            || is_non_title_seed_line(&prev.text)
        {
            break;
        }
        let h_ratio = prev.h.min(seed_h) / prev.h.max(seed_h);
        let gap = (curr.y - prev.bottom).max(0.0);
        if h_ratio >= 0.82 && gap <= 1.35 * prev.h {
            start_idx = prev_idx;
        } else {
            break;
        }
    }

    // Expand downward across wrapped title lines (2..=5 lines)
    while end_idx + 1 < lines.len() && (end_idx - start_idx) < 4 {
        let prev = &lines[end_idx];
        let next = &lines[end_idx + 1];
        if next.y >= header_limit_y {
            break;
        }
        let next_low = next.text.to_ascii_lowercase();
        if is_body_boundary_line(&next_low)
            || is_running_journal_header(&next_low)
            || looks_like_affiliation_line(&next.text)
            || next_low.starts_with("by ")
            || next_low.starts_with("dedicated to ")
            || next_low.starts_with("communicated by ")
            || next_low.starts_with("received ")
        {
            break;
        }

        let h_ratio = next.h.min(seed_h) / next.h.max(seed_h);
        let gap = (next.y - prev.bottom).max(0.0);
        let baseline_step = next.y - prev.y;

        // If we already have >= 2 title lines, check consistency with intra-title line step
        let step_consistent = if end_idx > start_idx {
            let prev_step = (lines[end_idx].y - lines[end_idx - 1].y).max(0.008);
            baseline_step <= 1.45 * prev_step
        } else {
            gap <= 1.35 * prev.h && baseline_step <= 2.45 * prev.h
        };

        let strong_continuation =
            ends_with_title_connector(&prev.text) || starts_with_title_connector(&next.text);

        if h_ratio < 0.80 || (!step_consistent && !strong_continuation) {
            break;
        }

        // Guard against same-font-size author line right below the title
        if !strong_continuation && looks_like_author_line(&next.text) && gap > 0.55 * prev.h {
            break;
        }

        end_idx += 1;
    }

    let raw_title_slices: Vec<&str> = (start_idx..=end_idx)
        .map(|i| lines[i].text.as_str())
        .collect();
    let mut assembled_title = join_title_lines(&raw_title_slices);
    // Clean trailing footnote markers like "*" on titles ("Optimal Approximations*")
    assembled_title = assembled_title
        .trim_end_matches(['*', '†', '‡'])
        .trim()
        .to_string();

    let title_out = if assembled_title.len() >= 4 && !looks_like_garbage_title(&assembled_title) {
        Some(assembled_title)
    } else {
        None
    };

    // Extract authors from header lines immediately below the title block
    let title_bottom = lines[end_idx].bottom;
    let mut authors = Vec::new();
    for ln in lines.iter().skip(end_idx + 1) {
        if ln.y >= header_limit_y || (ln.y - title_bottom) > 0.24 {
            break;
        }
        let low = ln.text.to_ascii_lowercase();
        if is_body_boundary_line(&low) || is_editorial_board_heading(&low) {
            break;
        }
        if low.starts_with("dedicated to ")
            || low.starts_with("in memory of ")
            || low.starts_with("communicated by ")
            || low.starts_with("received ")
            || is_running_journal_header(&low)
            || is_book_series_or_publisher_banner(&low)
            || looks_like_affiliation_line(&ln.text)
        {
            continue;
        }
        // Stop if we hit a dense body/abstract paragraph
        if ln.text.len() > 130 && !ln.text.contains(',') && !low.contains(" and ") {
            break;
        }
        if looks_like_author_line(&ln.text) {
            for a in split_author_list(&ln.text) {
                push_unique_author(&mut authors, a);
            }
        }
    }

    (title_out, authors)
}

#[derive(Default)]
struct FilenameParsed {
    doi: Option<String>,
    pii: Option<String>,
    arxiv_id: Option<String>,
    arxiv_version: Option<String>,
    isbn: Option<String>,
    author: Option<String>,
    year: Option<i64>,
    title: Option<String>,
    keywords: Vec<String>,
}

fn strip_browser_copy_suffix(stem: &str) -> &str {
    static COPY_SUFFIX_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let re = COPY_SUFFIX_RE.get_or_init(|| Regex::new(r"\s*\(\d{1,2}\)$").unwrap());
    if let Some(m) = re.find(stem) {
        stem[..m.start()].trim()
    } else {
        stem
    }
}

fn parse_filename_metadata(stem: &str) -> FilenameParsed {
    let mut out = FilenameParsed::default();
    let trimmed = strip_browser_copy_suffix(stem.trim());
    if trimmed.is_empty() {
        return out;
    }

    // 0. Publisher canonical download filenames:
    //    - ScienceDirect PII: `1-s2.0-S0021999118307125-main` or `S0022-1236(02)00045-2`
    //    - SpringerLink article ID: `s00208-019-01831-y` or archival `BF01390012` -> `10.1007/...`
    //    - Sanitized DOI stem: `10.1090_S0002-9947-2014-06012-3` or `10.1002%2Fcpa.3160420503`
    let (fn_pii, fn_pii_doi) = extract_pii_and_doi_from_text(trimmed);
    if fn_pii.is_some() {
        out.pii = fn_pii;
        out.doi = fn_pii_doi;
        return out;
    }
    static FN_SPRINGER_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let springer_re = FN_SPRINGER_RE.get_or_init(|| {
        Regex::new(r"^(?:(?i)(s\d{5}-\d{3}-\d{4,5}-[\dxzy])|(BF\d{7,8}))$").unwrap()
    });
    if let Some(c) = springer_re.captures(trimmed) {
        let suffix = c.get(1).or_else(|| c.get(2)).map(|m| m.as_str()).unwrap_or("");
        if !suffix.is_empty() {
            out.doi = normalize_doi(&format!("10.1007/{suffix}"));
            return out;
        }
    }
    static FN_ENCODED_DOI_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let enc_doi_re = FN_ENCODED_DOI_RE
        .get_or_init(|| Regex::new(r"(?i)^(10\.\d{4,9})(?:_|%2f)([^\s]+)$").unwrap());
    if let Some(c) = enc_doi_re.captures(trimmed) {
        if let (Some(pref), Some(suf)) = (c.get(1), c.get(2)) {
            if let Some(d) = normalize_doi(&format!("{}/{}", pref.as_str(), suf.as_str())) {
                out.doi = Some(d);
                return out;
            }
        }
    }

    // 1. Check arXiv new-style ID in filename (e.g. "2301.12345v2", "arXiv_2301.12345_Title")
    static FN_ARXIV_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let fn_arxiv = FN_ARXIV_RE
        .get_or_init(|| Regex::new(r"(?i)(?:^|[^0-9])(\d{4}\.\d{4,5})(v\d+)?(?:$|[^0-9])").unwrap());
    if let Some(c) = fn_arxiv.captures(trimmed) {
        if let Some(id_m) = c.get(1) {
            let cand = id_m.as_str();
            if infer_year_from_arxiv_id(cand).is_some() {
                out.arxiv_id = Some(cand.to_string());
                out.arxiv_version = c.get(2).map(|m| m.as_str().to_ascii_lowercase());
            }
        }
    }

    // 2. Check ISBN-13 in filename (e.g. "978-0-387-90244-9_Gilbarg_Trudinger")
    static FN_ISBN13_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let fn_isbn13 = FN_ISBN13_RE
        .get_or_init(|| Regex::new(r"(97[89][-\s]?(?:\d[-\s]?){9}\d)").unwrap());
    if let Some(c) = fn_isbn13.captures(trimmed) {
        if let Some(m) = c.get(1) {
            out.isbn = normalize_isbn(m.as_str());
        }
    }

    // 3. Check compact "SurnameYYYY" / "Surname_YYYY" / "Surname_YYYY_Title_Words" / "YYYY - Surname - Title"
    static SURNAME_YEAR_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let sy_re = SURNAME_YEAR_RE.get_or_init(|| {
        Regex::new(r"^([A-Za-zÀ-ÖØ-öø-ÿ][A-Za-zÀ-ÖØ-öø-ÿ'-]{1,28})[\s_.-]*\(?(19\d{2}|20[0-2]\d)[a-z]?\)?(?:[\s_.-]+(.*))?$").unwrap()
    });
    static YEAR_SURNAME_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let ys_re = YEAR_SURNAME_RE.get_or_init(|| {
        Regex::new(r"^(19\d{2}|20[0-2]\d)[a-z]?[\s_.-]+([A-Za-zÀ-ÖØ-öø-ÿ][A-Za-zÀ-ÖØ-öø-ÿ'-]{1,28})(?:[\s_.-]+(.*))?$").unwrap()
    });

    if let Some(c) = sy_re.captures(trimmed) {
        let raw_author = c.get(1).map(|m| m.as_str()).unwrap_or("");
        let low_author = raw_author.to_ascii_lowercase();
        if !matches!(
            low_author.as_str(),
            "arxiv" | "scan" | "paper" | "book" | "draft" | "version" | "vol" | "chapter" | "part"
        ) {
            out.author = Some(capitalize_surname(&split_camel_case(raw_author)));
            out.year = c.get(2).and_then(|m| m.as_str().parse::<i64>().ok());
            if let Some(rest) = c.get(3).map(|m| m.as_str().trim()).filter(|s| !s.is_empty()) {
                let cleaned_rest = clean_filename_to_title(rest);
                out.keywords = extract_significant_keywords(&cleaned_rest);
                let word_count = cleaned_rest.split_whitespace().count();
                if word_count >= 3 || (word_count >= 2 && cleaned_rest.len() >= 16) {
                    out.title = Some(cleaned_rest);
                }
            }
            return out;
        }
    } else if let Some(c) = ys_re.captures(trimmed) {
        let raw_author = c.get(2).map(|m| m.as_str()).unwrap_or("");
        out.year = c.get(1).and_then(|m| m.as_str().parse::<i64>().ok());
        out.author = Some(capitalize_surname(&split_camel_case(raw_author)));
        if let Some(rest) = c.get(3).map(|m| m.as_str().trim()).filter(|s| !s.is_empty()) {
            let cleaned_rest = clean_filename_to_title(rest);
            out.keywords = extract_significant_keywords(&cleaned_rest);
            let word_count = cleaned_rest.split_whitespace().count();
            if word_count >= 3 || (word_count >= 2 && cleaned_rest.len() >= 16) {
                out.title = Some(cleaned_rest);
            }
        }
        return out;
    }

    // 4. Fallback to Author_CamelCaseTitle / Author - Title (YYYY) / clean stem
    let (author, title, trailing_year) = parse_filename_author_and_title_with_year(trimmed);
    out.author = author;
    if out.year.is_none() {
        out.year = trailing_year;
    }
    if let Some(t) = title {
        if !looks_like_garbage_title(&t) {
            out.keywords = extract_significant_keywords(&t);
            out.title = Some(t);
        }
    }
    out
}

fn capitalize_surname(s: &str) -> String {
    let first_tok = s.split_whitespace().next().unwrap_or(s);
    let mut chars = first_tok.chars();
    match chars.next() {
        Some(f) => format!("{}{}", f.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn extract_significant_keywords(text: &str) -> Vec<String> {
    let mut kws = Vec::new();
    for tok in text.split(|c: char| !c.is_alphabetic()) {
        let low = tok.to_ascii_lowercase();
        if low.len() >= 4
            && !matches!(
                low.as_str(),
                "with" | "from" | "that" | "this" | "into" | "over" | "under" | "some" | "note" | "notes" | "et" | "al"
            )
            && !kws.contains(&low)
        {
            kws.push(low);
            if kws.len() >= 6 {
                break;
            }
        }
    }
    kws
}

fn split_camel_case(token: &str) -> String {
    let mut out = String::with_capacity(token.len() + 8);
    let chars: Vec<char> = token.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if i > 0 {
            let prev = chars[i - 1];
            let next_is_lower = chars.get(i + 1).is_some_and(|c| c.is_ascii_lowercase());
            if (prev.is_ascii_lowercase() && ch.is_ascii_uppercase())
                || (prev.is_ascii_uppercase() && ch.is_ascii_uppercase() && next_is_lower)
            {
                out.push(' ');
            }
        }
        out.push(ch);
    }
    out
}

fn is_non_author_filename_prefix(tok: &str) -> bool {
    let low = tok.trim().to_ascii_lowercase();
    looks_like_garbage_author(&low)
        || matches!(
            low.as_str(),
            "arxiv"
                | "scan"
                | "scanned"
                | "paper"
                | "book"
                | "draft"
                | "version"
                | "vol"
                | "chapter"
                | "part"
                | "document"
                | "doc"
                | "manuscript"
                | "article"
                | "main"
                | "preprint"
                | "final"
                | "fulltext"
                | "lecture"
                | "notes"
                | "thesis"
                | "slides"
                | "copy"
                | "acm"
                | "cvpr"
                | "iccv"
                | "icml"
                | "neurips"
                | "eccv"
                | "aaai"
                | "ijcai"
                | "iclr"
        )
}

#[cfg(test)]
fn parse_filename_author_and_title(stem: &str) -> (Option<String>, Option<String>) {
    let (au, ti, _) = parse_filename_author_and_title_with_year(stem);
    (au, ti)
}

fn split_filename_author_title_delimiter(stem: &str) -> Option<(&str, &str)> {
    for sep in [" - ", " – ", " — ", " -", "- "] {
        if let Some((l, r)) = stem.split_once(sep) {
            if !l.trim().is_empty() && !r.trim().is_empty() {
                return Some((l.trim(), r.trim()));
            }
        }
    }
    if let Some((l, r)) = stem.split_once('_') {
        if !l.trim().is_empty() && !r.trim().is_empty() {
            return Some((l.trim(), r.trim()));
        }
    }
    None
}

fn strip_trailing_year_from_title(raw: &str) -> (String, Option<i64>) {
    static PAREN_YEAR_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let paren_re = PAREN_YEAR_RE.get_or_init(|| {
        Regex::new(r"[\s_.-]*(?:\((18\d{2}|19\d{2}|20[0-3]\d)\)|\[(18\d{2}|19\d{2}|20[0-3]\d)\])\s*$")
            .unwrap()
    });
    let trimmed = raw.trim();
    if let Some(c) = paren_re.captures(trimmed) {
        let m_all = c.get(0).unwrap();
        let prefix = trimmed[..m_all.start()].trim();
        if prefix.chars().filter(|ch| ch.is_alphabetic()).count() >= 4 {
            let yr = c
                .get(1)
                .or_else(|| c.get(2))
                .and_then(|m| m.as_str().parse::<i64>().ok());
            return (prefix.to_string(), yr);
        }
    }

    static BARE_YEAR_RE: StdOnceLock<Regex> = StdOnceLock::new();
    let bare_re = BARE_YEAR_RE
        .get_or_init(|| Regex::new(r"[\s_.-]+(18\d{2}|19\d{2}|20[0-3]\d)\s*$").unwrap());
    if let Some(c) = bare_re.captures(trimmed) {
        let m_all = c.get(0).unwrap();
        let prefix = trimmed[..m_all.start()].trim();
        let last_word = prefix
            .split_whitespace()
            .last()
            .unwrap_or("")
            .to_ascii_lowercase();
        let part_of_title = matches!(
            last_word.as_str(),
            "in" | "of" | "since" | "from" | "to" | "until" | "after" | "before"
        );
        if !part_of_title && prefix.chars().filter(|ch| ch.is_alphabetic()).count() >= 4 {
            let yr = c.get(1).and_then(|m| m.as_str().parse::<i64>().ok());
            return (prefix.to_string(), yr);
        }
    }

    (trimmed.to_string(), None)
}

fn is_filename_initial_token(tok: &str) -> bool {
    let stripped = tok.trim_matches('.');
    if stripped.is_empty() {
        return false;
    }
    if stripped.chars().count() == 1 && stripped.chars().all(char::is_alphabetic) {
        return tok.ends_with('.') || stripped.chars().all(char::is_uppercase);
    }
    tok.contains('.')
        && stripped.chars().count() <= 3
        && stripped.chars().all(|c| c.is_alphabetic() || c == '.' || c == '-')
}

fn extract_filename_first_author(left: &str) -> Option<String> {
    let trimmed = left.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Single CamelCase/plain token like `GiganteLeopardi` or `Halmos`
    if !trimmed.contains(' ') && !trimmed.contains(',') && !trimmed.contains('.') {
        let spaced = split_camel_case(trimmed);
        return spaced
            .split_whitespace()
            .next()
            .filter(|w| {
                w.len() >= 2
                    && w.chars().all(|c| c.is_alphabetic() || c == '-' || c == '\'')
                    && !is_non_author_filename_prefix(w)
            })
            .map(capitalize_surname);
    }

    let first_coauthor = trimmed
        .split([';', '&'])
        .next()
        .unwrap_or(trimmed)
        .split(" and ")
        .next()
        .unwrap_or(trimmed)
        .split(" AND ")
        .next()
        .unwrap_or(trimmed)
        .trim();

    let comma_parts: Vec<&str> = first_coauthor
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if comma_parts.is_empty() {
        return None;
    }

    // Check `Surname, Given` (e.g. `Lang, Serge` or `De Giorgi, Ennio` or `Halmos, P. R.`)
    // vs `De Marco G., Mariconda C.` (where comma_parts[0] already ends with initial `G.`).
    let p0_tokens: Vec<&str> = comma_parts[0].split_whitespace().collect();
    let p0_has_initial = p0_tokens.iter().any(|t| is_filename_initial_token(t));
    if comma_parts.len() == 2 && !p0_has_initial {
        let p1_tokens: Vec<&str> = comma_parts[1].split_whitespace().collect();
        let p0_words_valid = !p0_tokens.is_empty()
            && p0_tokens.len() <= 3
            && p0_tokens.iter().all(|w| {
                w.chars().all(|c| c.is_alphabetic() || c == '-' || c == '\'')
                    && !is_non_author_filename_prefix(w)
            });
        let p1_looks_given = !p1_tokens.is_empty()
            && p1_tokens.len() <= 3
            && (p1_tokens.iter().all(|t| is_filename_initial_token(t))
                || p1_tokens.len() == 1);
        if p0_words_valid && p1_looks_given {
            return Some(format!("{}, {}", comma_parts[0], comma_parts[1]));
        }
    }

    let person = comma_parts[0];
    let tokens: Vec<&str> = person.split_whitespace().collect();
    let mut non_initials: Vec<&str> = Vec::new();
    let mut initials: Vec<&str> = Vec::new();
    for &t in &tokens {
        if is_filename_initial_token(t) {
            initials.push(t);
        } else {
            non_initials.push(t);
        }
    }

    if non_initials.is_empty() || non_initials.len() > 4 {
        return None;
    }
    let all_valid = non_initials.iter().all(|w| {
        let cw = w.trim_matches(['.', '-', '\'']);
        cw.chars().count() >= 2
            && cw.chars().all(|c| c.is_alphabetic() || c == '-' || c == '\'')
            && !is_non_author_filename_prefix(cw)
    });
    if !all_valid {
        return None;
    }

    if non_initials.len() == 1 {
        let fam = non_initials[0].trim_matches('.');
        if initials.is_empty() {
            return Some(fam.to_string());
        }
        return Some(format!("{fam}, {}", initials.join(" ")));
    }

    // If trailing token in `tokens` was an initial (e.g. `De Marco G.` or `Garcia Marquez G.`),
    // all `non_initials` belong to the family name!
    if tokens.last().is_some_and(|t| is_filename_initial_token(t)) {
        return Some(format!("{}, {}", non_initials.join(" "), initials.join(" ")));
    }

    Some(non_initials.join(" "))
}

fn parse_filename_author_and_title_with_year(
    stem: &str,
) -> (Option<String>, Option<String>, Option<i64>) {
    // Supports `GrettonBorgwardt_AKernelTwoSampleTest`, `Lang S. - Real and functional analysis (1993)`,
    // `G. Fasshauer - Kernel-based Approximation (2015)`, `Epstein L. -Relativity Visualized 1994`.
    if let Some((left, right)) = split_filename_author_title_delimiter(stem) {
        let first_author = extract_filename_first_author(left);
        let (right_no_yr, yr) = strip_trailing_year_from_title(right);
        let right_title = clean_filename_to_title(&right_no_yr);
        if first_author.is_some()
            && right_title.len() >= 4
            && !looks_like_garbage_title(&right_title)
        {
            return (first_author, Some(right_title), yr);
        }
    }
    let (stem_no_yr, yr) = strip_trailing_year_from_title(stem);
    let cleaned = clean_filename_to_title(&stem_no_yr);
    if looks_like_garbage_title(&cleaned) {
        (None, None, yr)
    } else {
        (None, Some(cleaned), yr)
    }
}

fn clean_filename_to_title(stem: &str) -> String {
    stem.replace(['_', '-', '.'], " ")
        .split_whitespace()
        .map(split_camel_case)
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn first_author_from_raw(raw: &str) -> String {
    split_author_list(raw)
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            raw.split([';', '&'])
                .next()
                .unwrap_or(raw)
                .split(" and ")
                .next()
                .unwrap_or(raw)
                .trim()
                .to_string()
        })
}

fn heuristic_first_page(page_text: &str) -> (Option<String>, Vec<String>, Option<i64>) {
    let all_raw_lines: Vec<String> = page_text
        .lines()
        .map(|l| collapse_letter_spaced_line(&clean_ot1_and_ligatures_preserve_case(l.trim())))
        .collect();

    // If a previous article's References finish near the top of this page and a new article's
    // Abstract / Introduction appears below, start after those citation lines.
    let mut start_line = 0usize;
    if let Some(ref_pos) = all_raw_lines
        .iter()
        .take(18)
        .position(|l| is_references_heading_line(&l.to_ascii_lowercase()))
    {
        if all_raw_lines
            .iter()
            .skip(ref_pos + 1)
            .any(|l| is_body_boundary_line(&l.to_ascii_lowercase()))
        {
            start_line = ref_pos + 1;
            while start_line < all_raw_lines.len()
                && looks_like_citation_item_line(&all_raw_lines[start_line])
            {
                start_line += 1;
            }
        }
    }

    let mut lines: Vec<&str> = all_raw_lines
        .iter()
        .skip(start_line)
        .map(String::as_str)
        .filter(|l| {
            !l.is_empty()
                && !l.to_ascii_lowercase().starts_with("arxiv:")
                && !l.to_ascii_lowercase().starts_with("doi:")
                && !l.to_ascii_lowercase().starts_with("pii:")
                && !l.chars().all(|c| c.is_ascii_digit() || c.is_whitespace())
        })
        .take(20)
        .collect();

    if lines.is_empty() {
        return (None, Vec::new(), None);
    }

    // Skip running journal headers, short page labels, or institutional banners at top
    while lines.first().is_some_and(|l| {
        let low = l.to_ascii_lowercase();
        l.len() < 5 || is_running_journal_header(&low) || is_non_title_seed_line(l)
    }) {
        lines.remove(0);
    }

    if lines.is_empty() {
        return (None, Vec::new(), extract_clean_year(page_text));
    }

    // Book title pages frequently place the author line ("Serge Lang") directly ABOVE the book title
    let mut pre_title_authors: Vec<String> = Vec::new();
    if lines.len() >= 2 && looks_like_author_line(lines[0]) {
        let second = lines[1];
        let second_low = second.to_ascii_lowercase();
        if !looks_like_author_line(second)
            && !is_body_boundary_line(&second_low)
            && !is_non_title_seed_line(second)
            && (contains_title_topic_word(second) || second.split_whitespace().count() >= 3)
        {
            pre_title_authors = split_author_list(lines[0]);
            lines.remove(0);
        }
    }

    // Reconstruct multi-line title (up to 4 lines) when connectors, lowercase continuations, or ALL-CAPS titles wrap
    let mut title_end = 0usize;
    while title_end + 1 < lines.len() && title_end < 3 {
        let prev = lines[title_end];
        let next = lines[title_end + 1];
        let next_low = next.to_ascii_lowercase();
        if is_body_boundary_line(&next_low)
            || is_running_journal_header(&next_low)
            || looks_like_affiliation_line(next)
            || next_low.starts_with("by ")
        {
            break;
        }
        let connector_wrap =
            ends_with_title_connector(prev) || starts_with_title_connector(next);
        let lowercase_continuation =
            next.chars().next().is_some_and(char::is_lowercase) && !looks_like_author_line(next);
        let all_caps_wrap = is_all_caps_text(prev)
            && is_all_caps_text(next)
            && !looks_like_author_line(next);

        if connector_wrap || lowercase_continuation || all_caps_wrap {
            title_end += 1;
        } else {
            break;
        }
    }

    let assembled = join_title_lines(&lines[..=title_end])
        .trim_end_matches(['*', '†', '‡'])
        .trim()
        .to_string();
    let title = if assembled.len() >= 4 && !looks_like_garbage_title(&assembled) {
        Some(assembled)
    } else {
        None
    };

    let mut authors = Vec::new();
    for a in pre_title_authors {
        push_unique_author(&mut authors, a);
    }
    for &ln in lines.iter().skip(title_end + 1).take(4) {
        let low = ln.to_ascii_lowercase();
        if is_body_boundary_line(&low)
            || is_editorial_board_heading(&low)
            || (ln.len() > 110 && !ln.contains(','))
        {
            break;
        }
        if looks_like_affiliation_line(ln)
            || is_book_series_or_publisher_banner(&low)
            || low.starts_with("dedicated to ")
            || low.starts_with("communicated by ")
            || low.starts_with("received ")
        {
            continue;
        }
        if looks_like_author_line(ln) {
            for a in split_author_list(ln) {
                push_unique_author(&mut authors, a);
            }
        }
    }

    let year = extract_clean_year(page_text);

    (title, authors, year)
}

fn extract_xmp_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start_idx = xml.find(&open)?;
    let after_open = &xml[start_idx..];
    let close_tag = format!("</{tag}>");
    let end_idx = after_open.find(&close_tag)?;
    let inner = &after_open[..end_idx];
    // Strip nested rdf:Alt / rdf:Seq / rdf:li tags
    let re = Regex::new(r"<[^>]+>").ok()?;
    let stripped = re.replace_all(inner, " ");
    let collapsed = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = clean_ot1_and_ligatures_preserve_case(collapsed.trim());
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::types::ExtractedSpan;

    #[test]
    fn extracts_identifiers_and_validates_isbn() {
        let page1 = "Optimal Approximations by Piecewise Smooth Functions\n\
                     David Mumford and Jayant Shah\n\
                     DOI: 10.1002/cpa.3160420503.\n\
                     arXiv:2301.12345v2 [math.AP] 15 Jan 2023\n\
                     MR0997568  Zbl 0682.49007\n";
        let page_last = "Springer ISBN 978-0-387-90244-9 printed in USA.";
        let res = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[page1],
            Some(page_last),
            10,
            Some("fallback_name"),
        );

        assert_eq!(res.doi.as_deref(), Some("10.1002/cpa.3160420503"));
        assert_eq!(res.arxiv_id.as_deref(), Some("2301.12345"));
        assert_eq!(res.arxiv_version.as_deref(), Some("v2"));
        assert_eq!(res.mr_number.as_deref(), Some("0997568"));
        assert_eq!(res.zbl_number.as_deref(), Some("0682.49007"));
        assert_eq!(res.isbn.as_deref(), Some("9780387902449"));
        assert_eq!(
            res.heuristic_title.as_deref(),
            Some("Optimal Approximations by Piecewise Smooth Functions")
        );
        assert_eq!(
            res.heuristic_first_author.as_deref(),
            Some("David Mumford")
        );
        assert_eq!(res.heuristic_year, Some(2023));
        // Invalid ISBN fails checksum
        assert!(normalize_isbn("978-0-387-90244-0").is_none());
    }

    #[test]
    fn strips_inline_xml_skips_journal_banners_and_parses_camel_case_stem() {
        let res_xml = extract_identifiers_and_heuristics(
            None,
            Some("On Lagrange and Hermite interpolation in R <Superscript> <Emphasis Type=\"Italic\">k </Emphasis> </Superscript>"),
            Some("Mariano Gasca"),
            &[],
            None,
            12,
            Some("GascaMaeztu_Interpolation"),
        );
        assert_eq!(
            res_xml.heuristic_title.as_deref(),
            Some("On Lagrange and Hermite interpolation in R k")
        );

        let jmlr_page = "Journal of Machine Learning Research 13 (2012) 723-773 Submitted 4/08; Revised 11/11; Published 3/12\n\
                         A Kernel Two-Sample Test\n\
                         Arthur Gretton, Karsten M. Borgwardt\n";
        let res_jmlr = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[jmlr_page],
            None,
            51,
            Some("GrettonBorgwardtRaschScholkopfSmola_AKernelTwoSampletTest"),
        );
        assert_eq!(
            res_jmlr.heuristic_title.as_deref(),
            Some("A Kernel Two-Sample Test")
        );
        assert_eq!(
            res_jmlr.heuristic_first_author.as_deref(),
            Some("Arthur Gretton")
        );

        let (author, title) =
            parse_filename_author_and_title("GiganteLeopardi_SphericalDesignsWithLargeStrength");
        assert_eq!(author.as_deref(), Some("Gigante"));
        assert_eq!(
            title.as_deref(),
            Some("Spherical Designs With Large Strength")
        );
    }

    #[test]
    fn reconstructs_multiline_title_and_authors_from_page1_layout_and_ignores_msc_year() {
        let page1 = ExtractedPage {
            page_ord: 1,
            printed_label: None,
            width_pts: 612.0,
            height_pts: 792.0,
            raw_text: "RAIRO Analyse numérique (1974)\n\
                   On the Existence, Uniqueness and Approximation\n\
                   of Saddle-Point Problems Arising from\n\
                   Lagrangian Multipliers\n\
                   Franco Brezzi*\n\
                   Università di Pavia\n\
                   Abstract. We study saddle-point problems.\n\
                   2020 Mathematics Subject Classification. Primary 65N30.\n"
                .to_string(),
            spans: vec![
                ExtractedSpan {
                    text: "arXiv:9999.00000".to_string(),
                    bbox: NormalizedRect {
                        x: 0.02,
                        y: 0.25,
                        w: 0.02,
                        h: 0.35,
                    },
                },
                ExtractedSpan {
                    text: "Numerische Mathematik".to_string(),
                    bbox: NormalizedRect {
                        x: 0.12,
                        y: 0.05,
                        w: 0.30,
                        h: 0.011,
                    },
                },
                ExtractedSpan {
                    text: "On the Existence, Uniqueness and Approximation".to_string(),
                    bbox: NormalizedRect {
                        x: 0.14,
                        y: 0.13,
                        w: 0.72,
                        h: 0.022,
                    },
                },
                ExtractedSpan {
                    text: "of Saddle-Point Problems Arising from".to_string(),
                    bbox: NormalizedRect {
                        x: 0.18,
                        y: 0.158,
                        w: 0.64,
                        h: 0.022,
                    },
                },
                ExtractedSpan {
                    text: "Lagrangian Multipliers".to_string(),
                    bbox: NormalizedRect {
                        x: 0.28,
                        y: 0.186,
                        w: 0.44,
                        h: 0.022,
                    },
                },
                ExtractedSpan {
                    text: "Franco Brezzi*".to_string(),
                    bbox: NormalizedRect {
                        x: 0.38,
                        y: 0.24,
                        w: 0.24,
                        h: 0.015,
                    },
                },
                ExtractedSpan {
                    text: "Università di Pavia".to_string(),
                    bbox: NormalizedRect {
                        x: 0.35,
                        y: 0.265,
                        w: 0.30,
                        h: 0.012,
                    },
                },
                ExtractedSpan {
                    text: "Abstract. We study saddle-point problems.".to_string(),
                    bbox: NormalizedRect {
                        x: 0.12,
                        y: 0.33,
                        w: 0.76,
                        h: 0.012,
                    },
                },
            ],
        };

        // Pass page text that ONLY has "2020 Mathematics Subject Classification" so year extractor doesn't fall for 2020
        let text_only_msc = "On the Existence, Uniqueness and Approximation\n\
                             of Saddle-Point Problems Arising from\n\
                             Lagrangian Multipliers\n\
                             Franco Brezzi*\n\
                             2020 Mathematics Subject Classification. Primary 65N30.";

        let ids = extract_identifiers_and_heuristics_with_layout(
            None,
            Some("brezzi1974.dvi"), // garbage embedded title must be ignored
            None,
            &[text_only_msc],
            None,
            24,
            Some("brezzi1974"),
            Some(&page1),
        );

        assert_eq!(
            ids.heuristic_title.as_deref(),
            Some("On the Existence, Uniqueness and Approximation of Saddle-Point Problems Arising from Lagrangian Multipliers")
        );
        assert_eq!(ids.heuristic_first_author.as_deref(), Some("Franco Brezzi"));
        // Year 2020 from MSC ignored, fallback year 1974 recovered from filename "brezzi1974"!
        assert_eq!(ids.heuristic_year, Some(1974));
    }

    #[test]
    fn parses_scan_filenames_for_author_year_keywords_and_arxiv() {
        // Un-OCRed scan with empty page text and "brezzi1974.pdf"
        let scan_ids = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[],
            None,
            18,
            Some("brezzi1974"),
        );
        assert_eq!(scan_ids.heuristic_title, None);
        assert_eq!(scan_ids.heuristic_first_author.as_deref(), Some("Brezzi"));
        assert_eq!(scan_ids.heuristic_year, Some(1974));

        // Filename with keywords: "Brezzi_1974_Saddle.pdf"
        let kw_ids = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[],
            None,
            18,
            Some("Brezzi_1974_Saddle"),
        );
        assert_eq!(kw_ids.heuristic_title, None);
        assert_eq!(kw_ids.heuristic_first_author.as_deref(), Some("Brezzi"));
        assert_eq!(kw_ids.heuristic_year, Some(1974));
        assert_eq!(kw_ids.filename_keywords, vec!["saddle".to_string()]);

        // Filename with arXiv ID: "2301.12345v2.pdf"
        let ax_ids = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[],
            None,
            15,
            Some("2301.12345v2"),
        );
        assert_eq!(ax_ids.arxiv_id.as_deref(), Some("2301.12345"));
        assert_eq!(ax_ids.arxiv_version.as_deref(), Some("v2"));
        assert_eq!(ax_ids.heuristic_year, Some(2023));
    }

    #[test]
    fn extracts_complex_page1_dois_and_ignores_bibliography_citations_and_stale_pdf_headers() {
        // 1. Elsevier SICI DOI with balanced parentheses + glued "Received" footer
        let p1_elsevier = "Journal of Functional Analysis 190 (2002) 1–42\n\
                           Sobolev Embeddings on Metric Measure Spaces\n\
                           Piotr Hajlasz\n\
                           doi:10.1016/S0022-1236(02)00045-2Received 10 May 2001";
        let ids_els = extract_identifiers_and_heuristics(None, None, None, &[p1_elsevier], None, 42, None);
        assert_eq!(ids_els.doi.as_deref(), Some("10.1016/s0022-1236(02)00045-2"));

        // 2. Line-wrapped AMS DOI across a hyphen + Unicode en-dash
        let p1_wrapped = "Transactions of the American Mathematical Society\n\
                          Regularity of Free Boundaries in Two Phases\n\
                          Daniela De Silva\n\
                          https://doi.org/10.1090/S0002\u{2013}9947-\n2014-06012-3";
        let ids_wrap = extract_identifiers_and_heuristics(None, None, None, &[p1_wrapped], None, 30, None);
        assert_eq!(ids_wrap.doi.as_deref(), Some("10.1090/s0002-9947-2014-06012-3"));

        // 3. Modern arXiv DataCite DOI (10.48550/arXiv.*) automatically populates arxiv_id
        let p1_arxiv_doi = "Sharp Spectral Gaps on Hyperbolic Surfaces\n\
                             Nalini Anantharaman\n\
                            https://doi.org/10.48550/arXiv.2304.09876";
        let ids_ax = extract_identifiers_and_heuristics(None, None, None, &[p1_arxiv_doi], None, 22, None);
        assert_eq!(ids_ax.doi.as_deref(), Some("10.48550/arxiv.2304.09876"));
        assert_eq!(ids_ax.arxiv_id.as_deref(), Some("2304.09876"));

        // 4. Classic paper WITHOUT own DOI, citing DOIs/MRs/arXiv/ISBN in References on last page
        //    AND having a stale PDF /Title + /Author copied from an older LaTeX template!
        let p1_classic = "Mixed Finite Elements for Second Order Elliptic Problems\n\
                          Douglas N. Arnold and Franco Brezzi\n\
                          Abstract. We analyze mixed formulations.";
        let p_biblio = "References\n\
                        [1] J. Smith, Quantum Chaos on Graphs, doi:10.1007/s00220-020-03800-1, MR4123456, arXiv:2001.00001.\n\
                        [2] R. Hartshorne, Algebraic Geometry, ISBN 978-0-387-90244-9.";
        let ids_classic = extract_identifiers_and_heuristics(
            None,
            Some("Old Unrelated LaTeX Template Title on Fluid Mechanics"),
            Some("Wrong Stale Template Author"),
            &[p1_classic],
            Some(p_biblio),
            20,
            None,
        );
        assert_eq!(ids_classic.doi, None);
        assert_eq!(ids_classic.mr_number, None);
        assert_eq!(ids_classic.arxiv_id, None);
        assert_eq!(ids_classic.isbn, None);
        assert!(!ids_classic.looks_like_book);
        assert_eq!(
            ids_classic.heuristic_title.as_deref(),
            Some("Mixed Finite Elements for Second Order Elliptic Problems")
        );
        assert_eq!(
            ids_classic.heuristic_first_author.as_deref(),
            Some("Douglas N. Arnold")
        );
    }

    #[test]
    fn handles_repository_cover_pages_publisher_filenames_ligatures_and_split_issue_pages() {
        // 1. JSTOR cover page on Page 1 + real article layout with OT1 ligature on Page 2
        let jstor_p1 = "Downloaded from https://www.jstor.org/stable/1970079\n\
                        Author(s): John Nash\n\
                        Your use of the JSTOR archive indicates your acceptance of the Terms & Conditions.";
        let jstor_p2_text = "Annals of Mathematics\n\
                             C1 Isometric Imbeddings of ﬁnite Manifolds\n\
                             By John Nash\n\
                             1. Introduction\n";
        let jstor_p2_layout = ExtractedPage {
            page_ord: 2,
            printed_label: None,
            width_pts: 612.0,
            height_pts: 792.0,
            raw_text: jstor_p2_text.to_string(),
            spans: vec![
                ExtractedSpan {
                    text: "C1 Isometric Imbeddings of ﬁnite Manifolds".to_string(),
                    bbox: NormalizedRect {
                        x: 0.16,
                        y: 0.18,
                        w: 0.68,
                        h: 0.022,
                    },
                },
                ExtractedSpan {
                    text: "By John Nash".to_string(),
                    bbox: NormalizedRect {
                        x: 0.38,
                        y: 0.23,
                        w: 0.24,
                        h: 0.015,
                    },
                },
                ExtractedSpan {
                    text: "1. Introduction".to_string(),
                    bbox: NormalizedRect {
                        x: 0.14,
                        y: 0.31,
                        w: 0.30,
                        h: 0.014,
                    },
                },
            ],
        };
        let ids_jstor = extract_identifiers_and_heuristics_with_front_pages(
            None,
            None,
            None,
            &[jstor_p1, jstor_p2_text],
            None,
            18,
            None,
            None,
            Some(&jstor_p2_layout),
        );
        assert_eq!(ids_jstor.doi.as_deref(), Some("10.2307/1970079"));
        assert_eq!(
            ids_jstor.heuristic_title.as_deref(),
            Some("C1 Isometric Imbeddings of finite Manifolds")
        );
        assert_eq!(ids_jstor.heuristic_first_author.as_deref(), Some("John Nash"));

        // 2. ScienceDirect PII & SpringerLink download filenames (including browser `(1)` copy suffix)
        let ids_sd = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[],
            None,
            25,
            Some("1-s2.0-S0021999118307125-main (1)"),
        );
        assert_eq!(ids_sd.pii.as_deref(), Some("S0021999118307125"));

        let ids_sp_mod = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[],
            None,
            25,
            Some("s00208-019-01831-y"),
        );
        assert_eq!(ids_sp_mod.doi.as_deref(), Some("10.1007/s00208-019-01831-y"));

        let ids_sp_bf = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[],
            None,
            25,
            Some("BF01390012"),
        );
        assert_eq!(ids_sp_bf.doi.as_deref(), Some("10.1007/bf01390012"));

        // 3. Split-issue archival page where previous article's References end near top of Page 1,
        //    followed by letter-spaced title & OT1-accented author ("J.-C. N ´ ed ´ elec")
        let split_p1 = "References\n\
                        [8] P. Lax, Hyperbolic Systems, 1957.\n\
                        O N   M I X E D   F I N I T E   E L E M E N T S\n\
                        J.-C. N ´ ed ´ elec\n\
                        Abstract. We construct H(curl) conforming elements in R3.";
        let ids_split = extract_identifiers_and_heuristics(None, None, None, &[split_p1], None, 22, None);
        assert_eq!(
            ids_split.heuristic_title.as_deref(),
            Some("ON MIXED FINITE ELEMENTS")
        );
        assert_eq!(
            ids_split.heuristic_first_author.as_deref(),
            Some("J.-C. Nedelec")
        );

        // 4. Math-formula-stripped prose core fallback title
        assert_eq!(
            extract_prose_core_title("C 1 , α regularity for p(x)-Laplacian systems in R n")
                .as_deref(),
            Some("regularity for Laplacian systems in")
        );
    }

    #[test]
    fn skips_book_series_half_title_editorial_board_and_parses_book_filenames() {
        let p1_half_title = "Graduate Texts in Mathematics 142\n\
                             Editorial Board\n\
                             S. Axler F.W. Gehring K.A. Ribet\n\
                             Springer";
        let p2_series_list = "Publishers’ page\nThis page intentionally left blank";
        let p3_title_page = "Serge Lang\n\
                             Real and Functional Analysis\n\
                             Third Edition\n\
                             Springer-Verlag 1993";
        let p4_copyright = "ISBN 978-0-387-94001-4\n© 1993 Springer-Verlag New York, Inc.";

        let ids_gtm = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[p1_half_title, p2_series_list, p3_title_page, p4_copyright],
            None,
            596,
            Some("Lang S. - Real and functional analysis (1993)"),
        );
        assert!(ids_gtm.looks_like_book);
        assert_eq!(ids_gtm.isbn.as_deref(), Some("9780387940014"));
        assert_eq!(
            ids_gtm.heuristic_title.as_deref(),
            Some("Real and Functional Analysis")
        );
        assert_eq!(
            ids_gtm
                .heuristic_first_author
                .as_deref()
                .and_then(clean_author_surname)
                .as_deref(),
            Some("Lang")
        );
        assert_eq!(ids_gtm.heuristic_year, Some(1993));

        // Filename-only book parsing cases
        let ids_init_first = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[],
            None,
            520,
            Some("G. Fasshauer - Kernel-based Approximation Methods using MATLAB (2015)"),
        );
        assert_eq!(
            ids_init_first
                .heuristic_first_author
                .as_deref()
                .and_then(clean_author_surname)
                .as_deref(),
            Some("Fasshauer")
        );
        assert_eq!(
            ids_init_first.heuristic_title.as_deref(),
            Some("Kernel based Approximation Methods using MATLAB")
        );
        assert_eq!(ids_init_first.heuristic_year, Some(2015));

        let ids_multi_fam = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[],
            None,
            340,
            Some("De Marco G., Mariconda C. - Esercizi in una variabile (2015) "),
        );
        assert_eq!(
            ids_multi_fam
                .heuristic_first_author
                .as_deref()
                .and_then(clean_author_surname)
                .as_deref(),
            Some("De Marco")
        );
        assert_eq!(
            ids_multi_fam.heuristic_title.as_deref(),
            Some("Esercizi in una variabile")
        );
        assert_eq!(ids_multi_fam.heuristic_year, Some(2015));

        let ids_tight_dash = extract_identifiers_and_heuristics(
            None,
            None,
            None,
            &[],
            None,
            210,
            Some("Epstein L. -Relativity Visualized 1994"),
        );
        assert_eq!(
            ids_tight_dash
                .heuristic_first_author
                .as_deref()
                .and_then(clean_author_surname)
                .as_deref(),
            Some("Epstein")
        );
        assert_eq!(
            ids_tight_dash.heuristic_title.as_deref(),
            Some("Relativity Visualized")
        );
        assert_eq!(ids_tight_dash.heuristic_year, Some(1994));
    }

    #[test]
    fn italian_collana_skips_series_banners_and_catalog_isbns_to_find_page5_title() {
        let p1 = "Il Mondo Matematico\nCollana diretta da Comitato Scientifico\nEdizione speciale per RBA Italia";
        let p2 = "Volumi pubblicati nella stessa collana:\n1. ISBN 978-0-387-90244-9\n2. ISBN 978-0-521-79540-1\n3. ISBN 978-0-387-95385-4\n4. ISBN 978-0-387-98592-3";
        let p3 = "Presentazione della collana\nTutti i diritti riservati.";
        let p4 = "Copyright © 2011 RBA Contenidos Editoriales\nISBN 978-0-387-94705-1\nFinito di stampare in Italia";
        let p5 = "Enrique Gracián\nI numeri primi\nUn lungo cammino verso l'infinito";

        let ids = extract_identifiers_and_heuristics(
            None,
            Some("Il Mondo Matematico"),
            None,
            &[p1, p2, p3, p4, p5],
            None,
            152,
            None,
        );
        assert_eq!(ids.isbn.as_deref(), Some("9780387947051"));
        assert!(
            ids.heuristic_title
                .as_deref()
                .is_some_and(|t| t.starts_with("I numeri primi")),
            "expected page-5 book title, got {:?}",
            ids.heuristic_title
        );
        assert_eq!(
            ids.heuristic_first_author
                .as_deref()
                .and_then(clean_author_surname)
                .as_deref(),
            Some("Gracián")
        );
    }
}
