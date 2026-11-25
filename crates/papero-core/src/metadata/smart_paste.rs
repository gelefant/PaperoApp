use crate::bibtex::parse_bibtex_entries;
use crate::metadata::extract::{extract_doi_from_text, normalize_isbn};
use crate::metadata::models::{
    extract_arxiv_from_doi, normalize_arxiv_base_id, normalize_mr_digits, BibliographicRecord,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SmartPasteResult {
    Bibtex {
        records: Vec<BibliographicRecord>,
    },
    Identifiers {
        doi: Option<String>,
        arxiv_id: Option<String>,
        mr_number: Option<String>,
        zbl_number: Option<String>,
        isbn: Option<String>,
    },
    Empty,
}

fn arxiv_any_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:arxiv\.org/(?:abs|pdf)/|\barxiv[:\s]+)?(\d{4}\.\d{4,5}|[a-z-]+(?:\.[a-z]{2})?/\d{7})(?:v\d+)?(?:\.pdf)?\b",
        )
        .unwrap()
    })
}

fn mr_any_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:mathscinet-getitem\?mr=|\bmr(?:\s*number)?[:#\s]*)0*(\d{5,7})\b")
            .unwrap()
    })
}

fn zbl_any_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:zbmath\.org/\?q=an:|zbmath\.org/|\b(?:zbl|zbmath)[:#\s]*)(\d{3,4}\.\d{5})\b")
            .unwrap()
    })
}

fn isbn_any_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(?:\bISBN(?:-1[03])?[:\s]*)?((?:97[89][-\s]?)?(?:\d[-\s]?){9}[\dXx])\b")
            .unwrap()
    })
}

/// Automatically detects and parses pasted BibTeX entries, DOIs, arXiv URLs/IDs, MR URLs/numbers,
/// zbMATH IDs/URLs, or ISBN-10/13 strings.
pub fn parse_smart_paste(raw: &str) -> SmartPasteResult {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return SmartPasteResult::Empty;
    }

    // 1. BibTeX detection (`@article{...`, `@book {...`, etc.)
    if trimmed.contains('@') && trimmed.contains('{') {
        let entries = parse_bibtex_entries(trimmed);
        if !entries.is_empty() {
            return SmartPasteResult::Bibtex {
                records: entries.into_iter().map(|e| e.record).collect(),
            };
        }
    }

    // 2. Identifiers & URLs
    let doi = extract_doi_from_text(trimmed);
    let mut arxiv_id = doi.as_deref().and_then(extract_arxiv_from_doi);
    if arxiv_id.is_none() {
        // Explicit arXiv URL/prefix or standalone YYMM.NNNNN / arch-ive/NNNNNNN
        let low = trimmed.to_ascii_lowercase();
        let has_arxiv_hint = low.contains("arxiv") || !trimmed.contains(' ');
        if has_arxiv_hint {
            if let Some(cap) = arxiv_any_re().captures(trimmed) {
                if let Some(m) = cap.get(1) {
                    // Avoid mistaking a bare DOI suffix or zbMATH number for an arXiv ID
                    let candidate = m.as_str();
                    let is_zbmath = low.contains("zbmath") || low.contains("zbl");
                    if (!is_zbmath && doi.is_none()) || low.contains("arxiv") {
                        arxiv_id = normalize_arxiv_base_id(candidate);
                    }
                }
            }
        }
    }

    let mr_number = mr_any_re()
        .captures(trimmed)
        .and_then(|c| c.get(1))
        .and_then(|m| normalize_mr_digits(m.as_str()));

    let zbl_number = zbl_any_re()
        .captures(trimmed)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    let isbn = if doi.is_none() && arxiv_id.is_none() && mr_number.is_none() && zbl_number.is_none()
    {
        isbn_any_re()
            .captures_iter(trimmed)
            .filter_map(|c| c.get(1))
            .find_map(|m| normalize_isbn(m.as_str()))
    } else if trimmed.to_ascii_lowercase().contains("isbn") {
        isbn_any_re()
            .captures_iter(trimmed)
            .filter_map(|c| c.get(1))
            .find_map(|m| normalize_isbn(m.as_str()))
    } else {
        None
    };

    if doi.is_none()
        && arxiv_id.is_none()
        && mr_number.is_none()
        && zbl_number.is_none()
        && isbn.is_none()
    {
        return SmartPasteResult::Empty;
    }

    SmartPasteResult::Identifiers {
        doi,
        arxiv_id,
        mr_number,
        zbl_number,
        isbn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_urls_ids_and_bibtex() {
        match parse_smart_paste("https://arxiv.org/abs/2301.12345v2") {
            SmartPasteResult::Identifiers { arxiv_id, .. } => {
                assert_eq!(arxiv_id.as_deref(), Some("2301.12345"));
            }
            other => panic!("expected arxiv id, got {other:?}"),
        }

        match parse_smart_paste("https://doi.org/10.1007/s00222-020-00999-1") {
            SmartPasteResult::Identifiers { doi, .. } => {
                assert_eq!(doi.as_deref(), Some("10.1007/s00222-020-00999-1"));
            }
            other => panic!("expected doi, got {other:?}"),
        }

        match parse_smart_paste("https://mathscinet.ams.org/mathscinet-getitem?mr=997568") {
            SmartPasteResult::Identifiers { mr_number, .. } => {
                assert_eq!(mr_number.as_deref(), Some("0997568"));
            }
            other => panic!("expected mr, got {other:?}"),
        }

        match parse_smart_paste("ISBN 978-0-387-94705-1") {
            SmartPasteResult::Identifiers { isbn, .. } => {
                assert_eq!(isbn.as_deref(), Some("9780387947051"));
            }
            other => panic!("expected isbn, got {other:?}"),
        }

        let bib = "@article{Erdos1975,\n  author = {Erdos, Paul},\n  title = {Graph Theory},\n  year = {1975}\n}";
        match parse_smart_paste(bib) {
            SmartPasteResult::Bibtex { records } => {
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].title, "Graph Theory");
                assert_eq!(records[0].year, Some(1975));
            }
            other => panic!("expected bibtex, got {other:?}"),
        }
    }
}
