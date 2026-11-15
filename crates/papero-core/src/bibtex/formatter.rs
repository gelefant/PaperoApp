use crate::bibtex::latex_escape::{format_bibtex_pages, format_bibtex_title, unicode_to_ams_latex};
use crate::metadata::models::{BibliographicRecord, WorkType};
use crate::text_norm::normalize_math_text;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BibtexExportOptions {
    /// Wrap non-URL/DOI lines around column ~72 with 14-space continuation indent like MRLookup.
    pub wrap_lines: bool,
    /// Prefix `% Source: ...` comment ONLY on reconstructed non-MR entries.
    pub include_source_comment: bool,
    /// Custom key override (`None` = `MR<number>` if MR number present, else `<FirstAuthor><Year>`).
    pub citation_key_override: Option<String>,
}

impl Default for BibtexExportOptions {
    fn default() -> Self {
        Self {
            wrap_lines: true,
            include_source_comment: true,
            citation_key_override: None,
        }
    }
}

pub fn default_citation_key(rec: &BibliographicRecord) -> String {
    if let Some(mr) = rec.mr_number.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return format!("MR{mr}");
    }
    let first_family = rec
        .authors
        .iter()
        .find(|a| a.role == "author")
        .or_else(|| rec.authors.first())
        .map(|a| normalize_math_text(&a.family_name))
        .unwrap_or_else(|| "anon".to_string());
    let key_base: String = first_family
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let base = if key_base.is_empty() {
        "Papero".to_string()
    } else {
        let mut cs = key_base.chars();
        match cs.next() {
            Some(c) => format!("{}{}", c.to_ascii_uppercase(), cs.as_str()),
            None => "Papero".to_string(),
        }
    };
    match rec.year {
        Some(y) => format!("{base}{y}"),
        None => base,
    }
}

/// Generates BibTeX in exact MRLookup style:
/// 1. If `raw_mrlookup_bibtex` is present, returns it verbatim untouched.
/// 2. Otherwise formats from structured fields with `@<type> {<key>,`,
///    10-char right-aligned UPPERCASE field names, canonical MRLookup field order,
///    AMS LaTeX diacritics, protected capitals in titles, and `--` pages.
pub fn format_mrlookup_bibtex(rec: &BibliographicRecord, opts: &BibtexExportOptions) -> String {
    if opts.citation_key_override.is_none() {
        if let Some(raw) = rec
            .raw_mrlookup_bibtex
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return raw.to_string();
        }
    }

    let key = opts
        .citation_key_override
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default_citation_key(rec));

    let entry_type = rec.work_type.as_str();
    let mut lines: Vec<String> = Vec::new();

    if opts.include_source_comment && rec.source != "mrlookup" {
        let label = match rec.license_note.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(lic) => format!("% Source: {} ({lic})", rec.source),
            None if !rec.source.trim().is_empty() => format!("% Source: {}", rec.source),
            None => "% Source: Papero metadata".to_string(),
        };
        lines.push(label);
    }

    lines.push(format!("@{entry_type} {{{key},"));

    let authors_str = format_authors_list(rec, "author");
    let editors_str = format_authors_list(rec, "editor");
    let title_str = format_bibtex_title(&rec.title);
    let pages_str = rec.pages.as_deref().map(format_bibtex_pages);
    let mr_val = rec
        .mr_number_display
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| rec.mr_number.clone());
    let url_val = rec.url.clone().or_else(|| {
        rec.doi
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|d| format!("https://doi.org/{d}"))
    });

    let fields_order: &[&str] = match rec.work_type {
        WorkType::Article => &[
            "AUTHOR",
            "TITLE",
            "NOTE",
            "JOURNAL",
            "FJOURNAL",
            "VOLUME",
            "YEAR",
            "NUMBER",
            "PAGES",
            "ISSN",
            "CODEN",
            "MRCLASS",
            "MRNUMBER",
            "MRREVIEWER",
            "DOI",
            "URL",
        ],
        WorkType::Book => &[
            "AUTHOR",
            "EDITOR",
            "TITLE",
            "SERIES",
            "VOLUME",
            "EDITION",
            "NOTE",
            "PUBLISHER",
            "ADDRESS",
            "YEAR",
            "PAGES",
            "ISBN",
            "MRCLASS",
            "MRNUMBER",
            "MRREVIEWER",
            "DOI",
            "URL",
        ],
        WorkType::Incollection | WorkType::Inproceedings => &[
            "AUTHOR",
            "TITLE",
            "BOOKTITLE",
            "SERIES",
            "VOLUME",
            "EDITOR",
            "PAGES",
            "PUBLISHER",
            "ADDRESS",
            "YEAR",
            "ISBN",
            "MRCLASS",
            "MRNUMBER",
            "MRREVIEWER",
            "DOI",
            "URL",
        ],
        WorkType::Phdthesis | WorkType::Misc => &[
            "AUTHOR",
            "TITLE",
            "NOTE",
            "PUBLISHER",
            "ADDRESS",
            "YEAR",
            "PAGES",
            "MRCLASS",
            "MRNUMBER",
            "MRREVIEWER",
            "DOI",
            "URL",
        ],
    };

    for &field in fields_order {
        let val: Option<String> = match field {
            "AUTHOR" => authors_str.clone(),
            "EDITOR" => editors_str.clone(),
            "TITLE" => (!title_str.trim().is_empty()).then_some(title_str.clone()),
            "BOOKTITLE" => rec.booktitle.as_deref().map(format_bibtex_title),
            "NOTE" => rec.note.as_deref().map(unicode_to_ams_latex),
            "JOURNAL" => rec.journal_abbrev.as_deref().map(unicode_to_ams_latex),
            "FJOURNAL" => rec.journal_full.as_deref().map(unicode_to_ams_latex),
            "SERIES" => rec.series.as_deref().map(unicode_to_ams_latex),
            "VOLUME" => rec.volume.clone(),
            "EDITION" => rec.edition.clone(),
            "PUBLISHER" => rec.publisher.as_deref().map(unicode_to_ams_latex),
            "ADDRESS" => rec.address.as_deref().map(unicode_to_ams_latex),
            "YEAR" => rec.year.map(|y| y.to_string()),
            "NUMBER" => rec.issue_number.clone(),
            "PAGES" => pages_str.clone(),
            "ISBN" => rec.isbn.clone(),
            "ISSN" => rec.issn.clone(),
            "CODEN" => rec.coden.clone(),
            "MRCLASS" => format_mrclass(rec),
            "MRNUMBER" => mr_val.clone(),
            "MRREVIEWER" => rec.mr_reviewer.as_deref().map(unicode_to_ams_latex),
            "DOI" => rec.doi.clone(),
            "URL" => url_val.clone(),
            _ => None,
        };

        if let Some(v) = val.filter(|s| !s.trim().is_empty()) {
            let allow_wrap = opts.wrap_lines
                && !matches!(
                    field,
                    "DOI" | "URL" | "ISSN" | "ISBN" | "MRNUMBER" | "YEAR" | "VOLUME" | "NUMBER" | "PAGES"
                );
            lines.push(format_field_line(field, &v, allow_wrap));
        }
    }

    lines.push("}".to_string());
    lines.join("\n")
}

fn format_authors_list(rec: &BibliographicRecord, role: &str) -> Option<String> {
    let list: Vec<String> = rec
        .authors
        .iter()
        .filter(|a| a.role == role)
        .map(|a| unicode_to_ams_latex(&a.format_bib()))
        .filter(|s| !s.is_empty())
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list.join(" and "))
    }
}

fn format_mrclass(rec: &BibliographicRecord) -> Option<String> {
    let primary = rec.msc_primary.as_deref()?.trim();
    if primary.is_empty() {
        return None;
    }
    if rec.msc_secondary.is_empty() {
        Some(primary.to_string())
    } else {
        Some(format!("{primary} ({})", rec.msc_secondary.join(" ")))
    }
}

fn format_field_line(field: &str, value: &str, wrap: bool) -> String {
    let prefix = format!("{field:>10} = {{");
    let full = format!("{prefix}{value}}},");
    if !wrap || full.len() <= 72 || !value.contains(' ') {
        return full;
    }

    // Wrap on word boundaries targeting <= 72 chars per line with 14-space indent
    let words: Vec<&str> = value.split_whitespace().collect();
    let mut lines = Vec::new();
    let mut cur = prefix;
    let mut first_word = true;

    for word in words {
        if first_word {
            cur.push_str(word);
            first_word = false;
        } else if cur.len() + 1 + word.len() <= 72 {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(cur);
            cur = format!("{}{word}", " ".repeat(14));
        }
    }
    cur.push_str("},");
    lines.push(cur);
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::models::BibliographicAuthor;

    #[test]
    fn formats_reconstructed_article_with_exact_10_char_alignment() {
        let rec = BibliographicRecord {
            work_type: WorkType::Article,
            title: "Optimal approximations by piecewise smooth functions".into(),
            authors: vec![
                BibliographicAuthor {
                    family_name: "Mumford".into(),
                    given_name: "David".into(),
                    role: "author".into(),
                },
                BibliographicAuthor {
                    family_name: "Shah".into(),
                    given_name: "Jayant".into(),
                    role: "author".into(),
                },
            ],
            journal_abbrev: Some("Comm. Pure Appl. Math.".into()),
            journal_full: Some("Communications on Pure and Applied Mathematics".into()),
            volume: Some("42".into()),
            issue_number: Some("5".into()),
            year: Some(1989),
            pages: Some("577-685".into()),
            issn: Some("0010-3640".into()),
            msc_primary: Some("49F20".into()),
            msc_secondary: vec!["41A30".into(), "92A27".into()],
            mr_number: Some("0997568".into()),
            mr_number_display: Some("0997568".into()),
            mr_reviewer: Some("Constantin Udriște".into()),
            doi: Some("10.1002/cpa.3160420503".into()),
            source: "zbmath".into(),
            license_note: Some("CC-BY-SA 4.0".into()),
            ..Default::default()
        };

        let out = format_mrlookup_bibtex(
            &rec,
            &BibtexExportOptions {
                wrap_lines: false,
                include_source_comment: false,
                citation_key_override: None,
            },
        );
        let expected = "\
@article {MR0997568,
    AUTHOR = {Mumford, David and Shah, Jayant},
     TITLE = {Optimal approximations by piecewise smooth functions},
   JOURNAL = {Comm. Pure Appl. Math.},
  FJOURNAL = {Communications on Pure and Applied Mathematics},
    VOLUME = {42},
      YEAR = {1989},
    NUMBER = {5},
     PAGES = {577--685},
      ISSN = {0010-3640},
   MRCLASS = {49F20 (41A30 92A27)},
  MRNUMBER = {0997568},
MRREVIEWER = {Constantin Udri{\\c{s}}te},
       DOI = {10.1002/cpa.3160420503},
       URL = {https://doi.org/10.1002/cpa.3160420503},
}";
        assert_eq!(out, expected);
    }
}
