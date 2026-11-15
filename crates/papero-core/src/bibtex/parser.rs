use crate::metadata::models::{
    normalize_doi, normalize_mr_digits, normalize_pages, BibliographicAuthor, BibliographicRecord,
    WorkType,
};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ParsedBibtexEntry {
    pub entry_type: String,
    pub key: String,
    pub raw_entry: String,
    pub fields: HashMap<String, String>,
    pub record: BibliographicRecord,
}

/// Parses `.bib` content into structured entries preserving verbatim `raw_entry` text.
pub fn parse_bibtex_entries(input: &str) -> Vec<ParsedBibtexEntry> {
    let mut entries = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'@' {
            i += 1;
            continue;
        }
        let start_idx = i;
        i += 1;
        let type_start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
            i += 1;
        }
        let entry_type = input[type_start..i].trim().to_ascii_lowercase();
        if entry_type.is_empty() || matches!(entry_type.as_str(), "comment" | "preamble" | "string") {
            continue;
        }

        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || (bytes[i] != b'{' && bytes[i] != b'(') {
            continue;
        }
        let close_delim = if bytes[i] == b'{' { b'}' } else { b')' };
        i += 1;

        // Read balanced body up to matching closing delimiter
        let body_start = i;
        let mut depth: i32 = 1;
        let mut in_quotes = false;
        while i < bytes.len() && depth > 0 {
            let b = bytes[i];
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == b'"' && depth == 1 {
                in_quotes = !in_quotes;
            } else if !in_quotes {
                if b == b'{' || (close_delim == b')' && b == b'(') {
                    depth += 1;
                } else if b == close_delim {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                } else if b == b'}' && depth > 1 {
                    depth -= 1;
                }
            }
            i += 1;
        }

        if depth != 0 {
            break;
        }
        let body = &input[body_start..i];
        let raw_entry = input[start_idx..=i].trim().to_string();
        i += 1;

        let (key, fields) = parse_entry_body(body);
        let record = build_record_from_fields(&entry_type, &fields, &raw_entry);
        entries.push(ParsedBibtexEntry {
            entry_type,
            key,
            raw_entry,
            fields,
            record,
        });
    }

    entries
}

fn parse_entry_body(body: &str) -> (String, HashMap<String, String>) {
    let mut fields = HashMap::new();
    let Some((raw_key, rest)) = body.split_once(',') else {
        return (body.trim().to_string(), fields);
    };
    let key = raw_key.trim().to_string();
    let bytes = rest.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let name_start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-') {
            i += 1;
        }
        let field_name = rest[name_start..i].trim().to_ascii_uppercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            continue;
        }
        i += 1; // skip '='
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }

        let val = if bytes[i] == b'{' {
            i += 1;
            let val_start = i;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'{' {
                    depth += 1;
                } else if bytes[i] == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                i += 1;
            }
            let s = rest[val_start..i].to_string();
            if i < bytes.len() && bytes[i] == b'}' {
                i += 1;
            }
            s
        } else if bytes[i] == b'"' {
            i += 1;
            let val_start = i;
            let mut depth = 0;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'{' {
                    depth += 1;
                } else if bytes[i] == b'}' && depth > 0 {
                    depth -= 1;
                } else if bytes[i] == b'"' && depth == 0 {
                    break;
                }
                i += 1;
            }
            let s = rest[val_start..i].to_string();
            if i < bytes.len() && bytes[i] == b'"' {
                i += 1;
            }
            s
        } else {
            let val_start = i;
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            rest[val_start..i].trim().to_string()
        };

        let collapsed = val.split_whitespace().collect::<Vec<_>>().join(" ");
        if !field_name.is_empty() && !collapsed.is_empty() {
            fields.insert(field_name, collapsed);
        }
    }

    (key, fields)
}

fn build_record_from_fields(
    entry_type: &str,
    fields: &HashMap<String, String>,
    raw_entry: &str,
) -> BibliographicRecord {
    let mut authors = Vec::new();
    if let Some(a) = fields.get("AUTHOR") {
        authors.extend(parse_bibtex_authors(a, "author"));
    }
    if let Some(ed) = fields.get("EDITOR") {
        authors.extend(parse_bibtex_authors(ed, "editor"));
    }

    let (msc_primary, msc_secondary) = fields
        .get("MRCLASS")
        .map(|s| parse_mrclass(s))
        .unwrap_or((None, Vec::new()));

    let mr_raw = fields.get("MRNUMBER").cloned();
    let mr_digits = mr_raw.as_deref().and_then(normalize_mr_digits);
    let is_mr = mr_digits.is_some();

    BibliographicRecord {
        work_type: WorkType::from_str_lossy(entry_type),
        title: clean_bibtex_text(fields.get("TITLE").map(String::as_str).unwrap_or("")),
        authors,
        journal_abbrev: fields.get("JOURNAL").map(|s| clean_bibtex_text(s)),
        journal_full: fields.get("FJOURNAL").map(|s| clean_bibtex_text(s)),
        booktitle: fields.get("BOOKTITLE").map(|s| clean_bibtex_text(s)),
        series: fields.get("SERIES").map(|s| clean_bibtex_text(s)),
        volume: fields.get("VOLUME").cloned(),
        issue_number: fields.get("NUMBER").cloned(),
        year: fields.get("YEAR").and_then(|y| {
            y.chars()
                .filter(|c| c.is_ascii_digit())
                .take(4)
                .collect::<String>()
                .parse::<i64>()
                .ok()
        }),
        pages: fields.get("PAGES").map(|p| normalize_pages(p)),
        edition: fields.get("EDITION").cloned(),
        note: fields.get("NOTE").map(|s| clean_bibtex_text(s)),
        publisher: fields.get("PUBLISHER").map(|s| clean_bibtex_text(s)),
        address: fields.get("ADDRESS").map(|s| clean_bibtex_text(s)),
        isbn: fields
            .get("ISBN")
            .map(|s| s.split(',').next().unwrap_or(s).trim().to_string()),
        issn: fields.get("ISSN").cloned(),
        coden: fields.get("CODEN").cloned(),
        doi: fields.get("DOI").and_then(|d| normalize_doi(d)),
        arxiv_id: fields
            .get("EPRINT")
            .or_else(|| fields.get("ARXIV"))
            .cloned(),
        arxiv_version: None,
        mr_number: mr_digits,
        mr_number_display: mr_raw,
        mr_reviewer: fields.get("MRREVIEWER").map(|s| clean_bibtex_text(s)),
        zbl_number: fields.get("ZBL").or_else(|| fields.get("ZBLNUMBER")).cloned(),
        url: fields.get("URL").cloned(),
        msc_primary,
        msc_secondary,
        abstract_text: fields.get("ABSTRACT").map(|s| clean_bibtex_text(s)),
        raw_mrlookup_bibtex: is_mr.then(|| raw_entry.to_string()),
        source: if is_mr {
            "mrlookup".to_string()
        } else {
            "bibtex_import".to_string()
        },
        license_note: None,
    }
}

pub fn parse_bibtex_authors(raw: &str, role: &str) -> Vec<BibliographicAuthor> {
    raw.split(" and ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|person| {
            let cleaned = clean_bibtex_text(person);
            if let Some((fam, giv)) = cleaned.split_once(',') {
                BibliographicAuthor {
                    family_name: fam.trim().to_string(),
                    given_name: giv.trim().to_string(),
                    role: role.to_string(),
                }
            } else {
                let parts: Vec<&str> = cleaned.split_whitespace().collect();
                if parts.len() >= 2 {
                    BibliographicAuthor {
                        family_name: parts.last().unwrap_or(&"").to_string(),
                        given_name: parts[..parts.len() - 1].join(" "),
                        role: role.to_string(),
                    }
                } else {
                    BibliographicAuthor {
                        family_name: cleaned,
                        given_name: String::new(),
                        role: role.to_string(),
                    }
                }
            }
        })
        .collect()
}

pub fn parse_mrclass(raw: &str) -> (Option<String>, Vec<String>) {
    let trimmed = raw.trim();
    if let Some((p, rest)) = trimmed.split_once('(') {
        let primary = p.trim().to_string();
        let sec = rest
            .trim_end_matches(')')
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect();
        (Some(primary), sec)
    } else if trimmed.is_empty() {
        (None, Vec::new())
    } else {
        (Some(trimmed.to_string()), Vec::new())
    }
}

/// Converts common AMS LaTeX accent sequences & protective braces into clean Unicode for UI storage.
pub fn clean_bibtex_text(raw: &str) -> String {
    let s = raw
        .replace(r"\ ", " ")
        .replace(r"{\c{s}}", "ş")
        .replace(r"\c{s}", "ş")
        .replace(r"{\c{c}}", "ç")
        .replace(r"\c{c}", "ç")
        .replace(r"{\H{o}}", "ő")
        .replace(r"\H{o}", "ő")
        .replace(r"{\v{c}}", "č")
        .replace(r"\v{c}", "č")
        .replace(r"{\v{C}}", "Č")
        .replace(r"\v{C}", "Č")
        .replace(r#"{\"a}"#, "ä")
        .replace(r#"\"a"#, "ä")
        .replace(r#"{\"o}"#, "ö")
        .replace(r#"\"o"#, "ö")
        .replace(r#"{\"u}"#, "ü")
        .replace(r#"\"u"#, "ü")
        .replace(r"{\'e}", "é")
        .replace(r"\'e", "é")
        .replace(r"{\'a}", "á")
        .replace(r"{\`e}", "è")
        .replace(r"{\l{}}", "ł")
        .replace(r"{\o{}}", "ø")
        .replace(r"{\ss}", "ß");
    s.replace(['{', '}'], "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_verbatim_mrlookup_entry() {
        let bib = include_str!("../../tests/golden_bibtex/01_art_mumford_shah.bib");
        let parsed = parse_bibtex_entries(bib);
        assert_eq!(parsed.len(), 1);
        let e = &parsed[0];
        assert_eq!(e.key, "MR997568");
        assert_eq!(e.record.mr_number.as_deref(), Some("0997568"));
        assert_eq!(e.record.mr_number_display.as_deref(), Some("997568"));
        assert_eq!(e.record.msc_primary.as_deref(), Some("49F20"));
        assert_eq!(e.record.msc_secondary, vec!["41A30", "92A27"]);
        assert_eq!(e.record.authors.len(), 2);
        assert_eq!(e.record.authors[0].family_name, "Mumford");
        assert_eq!(e.record.doi.as_deref(), Some("10.1002/cpa.3160420503"));
    }
}
