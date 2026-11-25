use crate::bibtex::latex_escape::{format_bibtex_pages, format_bibtex_title, unicode_to_ams_latex};
use crate::metadata::models::{BibliographicRecord, WorkType};
use crate::text_norm::normalize_math_text;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BibtexKeyPattern {
    /// `MR<number>` when MR number is present, otherwise `SurnameYear` (default).
    #[default]
    MrDefault,
    /// `SurnameYear` (e.g. `Mumford1989`).
    AuthorYear,
    /// `SurnameYearWord` (e.g. `Mumford1989Optimal`).
    AuthorYearWord,
    /// `surnameyear` lowercase (e.g. `mumford1989`).
    AuthorLowerYear,
}

impl BibtexKeyPattern {
    pub fn from_str_lossy(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "author_year" | "cognomeanno" => Self::AuthorYear,
            "author_year_word" | "cognomeannoparola" => Self::AuthorYearWord,
            "author_lower_year" | "cognomeanno_lower" => Self::AuthorLowerYear,
            _ => Self::MrDefault,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MrDefault => "mr_default",
            Self::AuthorYear => "author_year",
            Self::AuthorYearWord => "author_year_word",
            Self::AuthorLowerYear => "author_lower_year",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BibtexFormat {
    /// Classic MRLookup-aligned BibTeX (`JOURNAL`, `YEAR`, `ADDRESS`, ...).
    #[default]
    Bibtex,
    /// Modern BibLaTeX (`JOURNALTITLE`, `DATE`, `LOCATION`, `EPRINT`/`EPRINTTYPE`, ...).
    Biblatex,
}

impl BibtexFormat {
    pub fn from_str_lossy(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "biblatex" => Self::Biblatex,
            _ => Self::Bibtex,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bibtex => "bibtex",
            Self::Biblatex => "biblatex",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BibtexExportOptions {
    /// Wrap non-URL/DOI lines around column ~72 with 14-space continuation indent like MRLookup.
    pub wrap_lines: bool,
    /// Prefix `% Source: ...` comment ONLY on reconstructed non-MR entries.
    pub include_source_comment: bool,
    /// Custom key override (`None` = computed from `key_pattern`).
    pub citation_key_override: Option<String>,
    /// Citation key generation pattern.
    #[serde(default)]
    pub key_pattern: BibtexKeyPattern,
    /// Export dialect (`bibtex` vs `biblatex`).
    #[serde(default)]
    pub format: BibtexFormat,
}

impl Default for BibtexExportOptions {
    fn default() -> Self {
        Self {
            wrap_lines: true,
            include_source_comment: true,
            citation_key_override: None,
            key_pattern: BibtexKeyPattern::MrDefault,
            format: BibtexFormat::Bibtex,
        }
    }
}

pub fn default_citation_key(rec: &BibliographicRecord) -> String {
    compute_citation_key(rec, BibtexKeyPattern::MrDefault)
}

fn first_author_key_token(rec: &BibliographicRecord, capitalize: bool) -> String {
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
    if key_base.is_empty() {
        return if capitalize {
            "Papero".to_string()
        } else {
            "papero".to_string()
        };
    }
    if !capitalize {
        return key_base;
    }
    let mut cs = key_base.chars();
    match cs.next() {
        Some(c) => format!("{}{}", c.to_ascii_uppercase(), cs.as_str()),
        None => "Papero".to_string(),
    }
}

fn first_significant_title_word(title: &str) -> String {
    const STOP: &[&str] = &[
        "a", "an", "the", "on", "of", "in", "to", "for", "with", "by", "and", "or", "from", "at",
        "as", "into", "some", "notes", "about", "over", "under", "il", "lo", "la", "i", "gli",
        "le", "un", "una", "su", "di", "del", "della", "dei", "degli", "delle", "per", "con",
        "le", "les", "des", "du", "sur", "une", "der", "die", "das", "ein", "eine", "uber",
    ];
    let norm = normalize_math_text(title);
    let mut fallback = String::new();
    for raw_tok in norm.split(|c: char| !c.is_ascii_alphanumeric()) {
        if raw_tok.len() < 2 {
            continue;
        }
        if fallback.is_empty() {
            fallback = raw_tok.to_string();
        }
        if !STOP.contains(&raw_tok) && raw_tok.chars().any(|c| c.is_ascii_alphabetic()) {
            let mut cs = raw_tok.chars();
            return match cs.next() {
                Some(c) => format!("{}{}", c.to_ascii_uppercase(), cs.as_str()),
                None => String::new(),
            };
        }
    }
    let mut cs = fallback.chars();
    match cs.next() {
        Some(c) => format!("{}{}", c.to_ascii_uppercase(), cs.as_str()),
        None => String::new(),
    }
}

/// Computes a citation key according to `pattern`.
pub fn compute_citation_key(rec: &BibliographicRecord, pattern: BibtexKeyPattern) -> String {
    match pattern {
        BibtexKeyPattern::MrDefault => {
            if let Some(mr) = rec.mr_number.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                return format!("MR{mr}");
            }
            let base = first_author_key_token(rec, true);
            match rec.year {
                Some(y) => format!("{base}{y}"),
                None => base,
            }
        }
        BibtexKeyPattern::AuthorYear => {
            let base = first_author_key_token(rec, true);
            match rec.year {
                Some(y) => format!("{base}{y}"),
                None => base,
            }
        }
        BibtexKeyPattern::AuthorYearWord => {
            let base = first_author_key_token(rec, true);
            let word = first_significant_title_word(&rec.title);
            match rec.year {
                Some(y) => format!("{base}{y}{word}"),
                None => format!("{base}{word}"),
            }
        }
        BibtexKeyPattern::AuthorLowerYear => {
            let base = first_author_key_token(rec, false);
            match rec.year {
                Some(y) => format!("{base}{y}"),
                None => base,
            }
        }
    }
}

/// Formats a ready-to-paste `\cite{<key>}` command using `pattern`.
pub fn format_cite_command(rec: &BibliographicRecord, pattern: BibtexKeyPattern) -> String {
    format!("\\cite{{{}}}", compute_citation_key(rec, pattern))
}

/// Converts a 0-based collision index into a lowercase alphabetic suffix (`0 -> "a"`, `25 -> "z"`, `26 -> "aa"`).
fn alpha_suffix(mut idx: usize) -> String {
    let mut chars = Vec::new();
    loop {
        chars.push((b'a' + (idx % 26) as u8) as char);
        if idx < 26 {
            break;
        }
        idx = idx / 26 - 1;
    }
    chars.into_iter().rev().collect()
}

/// Disambiguates colliding citation keys deterministically by appending `a`, `b`, `c`, ...
/// ONLY to keys that appear more than once in `base_keys` (and ensuring the suffixed key never
/// collides with another existing key).
pub fn disambiguate_citation_keys(base_keys: &[String]) -> Vec<String> {
    use std::collections::{HashMap, HashSet};

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for k in base_keys {
        *counts.entry(k.as_str()).or_insert(0) += 1;
    }

    let mut used: HashSet<String> = HashSet::with_capacity(base_keys.len());
    let mut next_idx: HashMap<&str, usize> = HashMap::new();
    let mut out = Vec::with_capacity(base_keys.len());

    // Reserve all unique keys first so suffixed duplicates never collide with an unrelated unique key.
    for k in base_keys {
        if counts.get(k.as_str()).copied().unwrap_or(0) == 1 {
            used.insert(k.clone());
        }
    }

    for k in base_keys {
        if counts.get(k.as_str()).copied().unwrap_or(0) == 1 {
            out.push(k.clone());
            continue;
        }
        let counter = next_idx.entry(k.as_str()).or_insert(0);
        loop {
            let cand = format!("{k}{}", alpha_suffix(*counter));
            *counter += 1;
            if used.insert(cand.clone()) {
                out.push(cand);
                break;
            }
        }
    }
    out
}

fn rewrite_raw_bibtex_key(raw: &str, new_key: &str) -> Option<String> {
    let at_pos = raw.find('@')?;
    let brace_rel = raw[at_pos..].find('{')?;
    let brace_pos = at_pos + brace_rel;
    let comma_rel = raw[brace_pos + 1..].find(',')?;
    let comma_pos = brace_pos + 1 + comma_rel;
    let mut out = String::with_capacity(raw.len() + new_key.len());
    out.push_str(&raw[..brace_pos + 1]);
    out.push_str(new_key);
    out.push_str(&raw[comma_pos..]);
    Some(out)
}

/// Generates BibTeX in exact MRLookup style (or BibLaTeX when `opts.format == BibtexFormat::Biblatex`):
/// 1. In BibTeX mode, if `raw_mrlookup_bibtex` is present and the effective key matches default MR,
///    returns it verbatim untouched (or rewrites just the `@type {KEY,` header if custom key/pattern is used).
/// 2. Otherwise formats from structured fields with `@<type> {<key>,`,
///    10-char right-aligned UPPERCASE field names, canonical field order,
///    AMS LaTeX diacritics, protected capitals in titles, and `--` pages.
pub fn format_mrlookup_bibtex(rec: &BibliographicRecord, opts: &BibtexExportOptions) -> String {
    let computed_key = opts
        .citation_key_override
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| compute_citation_key(rec, opts.key_pattern));

    if opts.format == BibtexFormat::Bibtex {
        if let Some(raw) = rec
            .raw_mrlookup_bibtex
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if opts.citation_key_override.is_none()
                && opts.key_pattern == BibtexKeyPattern::MrDefault
            {
                return raw.to_string();
            }
            if let Some(rewritten) = rewrite_raw_bibtex_key(raw, &computed_key) {
                return rewritten;
            }
        }
    }

    let key = computed_key;

    let entry_type = if opts.format == BibtexFormat::Biblatex && rec.work_type == WorkType::Phdthesis
    {
        "thesis"
    } else {
        rec.work_type.as_str()
    };
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

    let fields_order: &[&str] = if opts.format == BibtexFormat::Biblatex {
        match rec.work_type {
            WorkType::Article => &[
                "AUTHOR",
                "TITLE",
                "NOTE",
                "JOURNALTITLE",
                "SHORTJOURNAL",
                "VOLUME",
                "DATE",
                "NUMBER",
                "PAGES",
                "ISSN",
                "EPRINT",
                "EPRINTTYPE",
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
                "LOCATION",
                "DATE",
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
                "LOCATION",
                "DATE",
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
                "LOCATION",
                "DATE",
                "PAGES",
                "EPRINT",
                "EPRINTTYPE",
                "MRCLASS",
                "MRNUMBER",
                "MRREVIEWER",
                "DOI",
                "URL",
            ],
        }
    } else {
        match rec.work_type {
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
        }
    };

    let has_arxiv = rec
        .arxiv_id
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());

    for &field in fields_order {
        let val: Option<String> = match field {
            "AUTHOR" => authors_str.clone(),
            "EDITOR" => editors_str.clone(),
            "TITLE" => (!title_str.trim().is_empty()).then_some(title_str.clone()),
            "BOOKTITLE" => rec.booktitle.as_deref().map(format_bibtex_title),
            "NOTE" => rec.note.as_deref().map(unicode_to_ams_latex),
            "JOURNAL" | "SHORTJOURNAL" => rec.journal_abbrev.as_deref().map(unicode_to_ams_latex),
            "FJOURNAL" => rec.journal_full.as_deref().map(unicode_to_ams_latex),
            "JOURNALTITLE" => rec
                .journal_full
                .as_deref()
                .or(rec.journal_abbrev.as_deref())
                .map(unicode_to_ams_latex),
            "SERIES" => rec.series.as_deref().map(unicode_to_ams_latex),
            "VOLUME" => rec.volume.clone(),
            "EDITION" => rec.edition.clone(),
            "PUBLISHER" => rec.publisher.as_deref().map(unicode_to_ams_latex),
            "ADDRESS" | "LOCATION" => rec.address.as_deref().map(unicode_to_ams_latex),
            "YEAR" | "DATE" => rec.year.map(|y| y.to_string()),
            "NUMBER" => rec.issue_number.clone(),
            "PAGES" => pages_str.clone(),
            "ISBN" => rec.isbn.clone(),
            "ISSN" => rec.issn.clone(),
            "CODEN" => rec.coden.clone(),
            "EPRINT" => rec.arxiv_id.clone(),
            "EPRINTTYPE" => has_arxiv.then(|| "arXiv".to_string()),
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
                    "DOI"
                        | "URL"
                        | "ISSN"
                        | "ISBN"
                        | "MRNUMBER"
                        | "YEAR"
                        | "DATE"
                        | "VOLUME"
                        | "NUMBER"
                        | "PAGES"
                        | "EPRINT"
                        | "EPRINTTYPE"
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
                key_pattern: BibtexKeyPattern::MrDefault,
                format: BibtexFormat::Bibtex,
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

    #[test]
    fn citation_key_patterns_disambiguation_and_biblatex_format() {
        let rec = BibliographicRecord {
            work_type: WorkType::Article,
            title: "On the Optimal Approximations of Functions".into(),
            authors: vec![BibliographicAuthor {
                family_name: "Erdős".into(),
                given_name: "Paul".into(),
                role: "author".into(),
            }],
            journal_full: Some("Annals of Mathematics".into()),
            year: Some(1975),
            mr_number: Some("0400001".into()),
            arxiv_id: Some("2301.12345".into()),
            ..Default::default()
        };
        assert_eq!(
            compute_citation_key(&rec, BibtexKeyPattern::MrDefault),
            "MR0400001"
        );
        assert_eq!(
            compute_citation_key(&rec, BibtexKeyPattern::AuthorYear),
            "Erdos1975"
        );
        assert_eq!(
            compute_citation_key(&rec, BibtexKeyPattern::AuthorYearWord),
            "Erdos1975Optimal"
        );
        assert_eq!(
            compute_citation_key(&rec, BibtexKeyPattern::AuthorLowerYear),
            "erdos1975"
        );
        assert_eq!(
            format_cite_command(&rec, BibtexKeyPattern::AuthorYear),
            "\\cite{Erdos1975}"
        );

        let keys = disambiguate_citation_keys(&[
            "Erdos1975".into(),
            "Erdos1975".into(),
            "Erdos1975a".into(),
            "Poincare1895".into(),
        ]);
        // `Erdos1975a` was unique so duplicates skip `a` and become `b`, `c`!
        assert_eq!(keys, vec!["Erdos1975b", "Erdos1975c", "Erdos1975a", "Poincare1895"]);

        let blx = format_mrlookup_bibtex(
            &rec,
            &BibtexExportOptions {
                wrap_lines: false,
                include_source_comment: false,
                citation_key_override: None,
                key_pattern: BibtexKeyPattern::AuthorYear,
                format: BibtexFormat::Biblatex,
            },
        );
        assert!(blx.contains("@article {Erdos1975,"));
        assert!(blx.contains("JOURNALTITLE = {Annals of Mathematics},"));
        assert!(blx.contains("DATE = {1975},"));
        assert!(blx.contains("EPRINT = {2301.12345},"));
        assert!(blx.contains("EPRINTTYPE = {arXiv},"));
    }
}
