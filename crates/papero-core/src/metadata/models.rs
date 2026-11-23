use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorkType {
    #[default]
    Article,
    Book,
    Incollection,
    Inproceedings,
    Phdthesis,
    Misc,
}

impl WorkType {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkType::Article => "article",
            WorkType::Book => "book",
            WorkType::Incollection => "incollection",
            WorkType::Inproceedings => "inproceedings",
            WorkType::Phdthesis => "phdthesis",
            WorkType::Misc => "misc",
        }
    }

    pub fn from_str_lossy(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "book" | "monograph" | "edited-book" => WorkType::Book,
            "incollection" | "book-chapter" | "chapter" => WorkType::Incollection,
            "inproceedings" | "proceedings-article" | "conference-paper" => WorkType::Inproceedings,
            "phdthesis" | "dissertation" | "thesis" => WorkType::Phdthesis,
            "misc" | "preprint" => WorkType::Misc,
            _ => WorkType::Article,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BibliographicAuthor {
    pub family_name: String,
    pub given_name: String,
    #[serde(default = "default_role")]
    pub role: String, // "author" | "editor"
}

fn default_role() -> String {
    "author".to_string()
}

impl BibliographicAuthor {
    pub fn format_bib(&self) -> String {
        let fam = self.family_name.trim();
        let giv = self.given_name.trim();
        if giv.is_empty() {
            fam.to_string()
        } else if fam.is_empty() {
            giv.to_string()
        } else {
            format!("{fam}, {giv}")
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BibliographicRecord {
    pub work_type: WorkType,
    pub title: String,
    pub authors: Vec<BibliographicAuthor>,
    pub journal_abbrev: Option<String>,
    pub journal_full: Option<String>,
    pub booktitle: Option<String>,
    pub series: Option<String>,
    pub volume: Option<String>,
    pub issue_number: Option<String>,
    pub year: Option<i64>,
    pub pages: Option<String>,
    pub edition: Option<String>,
    pub note: Option<String>,
    pub publisher: Option<String>,
    pub address: Option<String>,
    pub isbn: Option<String>,
    pub issn: Option<String>,
    pub coden: Option<String>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub arxiv_version: Option<String>,
    pub mr_number: Option<String>,
    pub mr_number_display: Option<String>,
    pub mr_reviewer: Option<String>,
    pub zbl_number: Option<String>,
    pub url: Option<String>,
    pub msc_primary: Option<String>,
    pub msc_secondary: Vec<String>,
    pub abstract_text: Option<String>,
    pub raw_mrlookup_bibtex: Option<String>,
    pub source: String,
    pub license_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateMatch {
    pub score: f32,
    pub source: String,
    pub record: BibliographicRecord,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractedIdentifiers {
    pub doi: Option<String>,
    #[serde(default)]
    pub pii: Option<String>,
    pub arxiv_id: Option<String>,
    pub arxiv_version: Option<String>,
    pub isbn: Option<String>,
    pub mr_number: Option<String>,
    pub zbl_number: Option<String>,
    pub heuristic_title: Option<String>,
    #[serde(default)]
    pub alt_heuristic_titles: Vec<String>,
    pub heuristic_first_author: Option<String>,
    #[serde(default)]
    pub heuristic_authors: Vec<String>,
    pub heuristic_year: Option<i64>,
    #[serde(default)]
    pub filename_keywords: Vec<String>,
    pub looks_like_book: bool,
    #[serde(default)]
    pub is_user_query: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadataView {
    pub document_id: i64,
    pub work_type: String,
    pub title: String,
    pub authors: Vec<BibliographicAuthor>,
    pub journal_abbrev: Option<String>,
    pub journal_full: Option<String>,
    pub booktitle: Option<String>,
    pub series: Option<String>,
    pub volume: Option<String>,
    pub issue_number: Option<String>,
    pub year: Option<i64>,
    pub pages: Option<String>,
    pub edition: Option<String>,
    pub publisher: Option<String>,
    pub address: Option<String>,
    pub isbn: Option<String>,
    pub issn: Option<String>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub arxiv_version: Option<String>,
    pub mr_number: Option<String>,
    pub mr_number_display: Option<String>,
    pub mr_reviewer: Option<String>,
    pub zbl_number: Option<String>,
    pub url: Option<String>,
    pub msc_primary: Option<String>,
    pub msc_primary_status: String,
    pub msc_secondary: Vec<String>,
    pub abstract_text: Option<String>,
    pub raw_mrlookup_bibtex: Option<String>,
    pub metadata_source: Option<String>,
    pub metadata_license_note: Option<String>,
    pub manual_locked_fields: Vec<String>,
    #[serde(default)]
    pub metadata_verified: bool,
    pub exclude_from_cloud_llm: bool,
    pub tags: Vec<String>,
    pub pending_candidates: Vec<CandidateRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateRow {
    pub id: i64,
    pub document_id: i64,
    pub source: String,
    pub match_score: f32,
    pub record: BibliographicRecord,
}

/// Normalizes a 7-digit MR number (`"997568"` or `"MR0997568"` -> `Some("0997568")`).
pub fn normalize_mr_digits(raw: &str) -> Option<String> {
    let stripped = raw.trim().strip_prefix("MR").or_else(|| raw.trim().strip_prefix("mr")).unwrap_or(raw.trim());
    let digits: String = stripped.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 7 {
        return None;
    }
    Some(format!("{:0>7}", digits))
}

/// Normalizes pages (`"577-685"` / `"577–685"` -> `"577--685"`).
pub fn normalize_pages(raw: &str) -> String {
    let s = raw.trim().replace(['–', '—', '−'], "--");
    if s.contains("--") {
        return s;
    }
    // Single ASCII hyphen between page tokens -> double hyphen
    if let Some((a, b)) = s.split_once('-') {
        let a_t = a.trim();
        let b_t = b.trim();
        if !a_t.is_empty()
            && !b_t.is_empty()
            && a_t.chars().any(|c| c.is_ascii_digit())
            && b_t.chars().any(|c| c.is_ascii_digit())
        {
            return format!("{a_t}--{b_t}");
        }
    }
    s
}

/// Normalizes a DOI to lowercase without URL prefixes, Unicode dash variants, glued footer words,
/// or unbalanced trailing punctuation (while preserving balanced parentheses/angle brackets in
/// Elsevier/Wiley SICI DOIs like `10.1016/s0022-1236(02)00045-2`).
pub fn normalize_doi(raw: &str) -> Option<String> {
    let mut normalized_chars = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' => {}
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}' => {
                normalized_chars.push('-');
            }
            _ => normalized_chars.push(ch),
        }
    }
    let mut s = normalized_chars.trim();
    loop {
        let low = s.to_ascii_lowercase();
        let mut stripped_any = false;
        for prefix in [
            "https://doi.org/",
            "http://doi.org/",
            "https://dx.doi.org/",
            "http://dx.doi.org/",
            "doi.org/",
            "dx.doi.org/",
            "doi:",
        ] {
            if low.starts_with(prefix) {
                s = s[prefix.len()..].trim();
                stripped_any = true;
                break;
            }
        }
        if !stripped_any {
            break;
        }
    }

    let collapsed = if let Some((registrant, suffix)) = s.split_once('/') {
        let reg_t = registrant.trim();
        let suf_t = suffix.trim_start();
        if reg_t.starts_with("10.") && !suf_t.is_empty() {
            format!("{reg_t}/{suf_t}")
        } else {
            s.to_string()
        }
    } else {
        return None;
    };

    let token = collapsed
        .split(|c: char| c.is_whitespace() || c == '©' || c == '"' || c == '\'')
        .next()
        .unwrap_or("")
        .trim();

    let mut cur = token.to_string();
    for suffix in [
        ".pdf",
        "/pdf",
        "/full",
        "/abstract",
        "received",
        "accepted",
        "published",
        "available",
        "copyright",
        "allrightsreserved",
    ] {
        if cur.len() > suffix.len() + 8 && cur.to_ascii_lowercase().ends_with(suffix) {
            cur.truncate(cur.len() - suffix.len());
            break;
        }
    }

    loop {
        let trimmed = cur.trim_end_matches(['.', ',', ';', ':', ']', '}', '/', '\\']);
        if trimmed.len() != cur.len() {
            cur = trimmed.to_string();
            continue;
        }
        if cur.ends_with(')') {
            let opens = cur.chars().filter(|c| *c == '(').count();
            let closes = cur.chars().filter(|c| *c == ')').count();
            if closes > opens {
                cur.pop();
                continue;
            }
        }
        if cur.ends_with('>') {
            let opens = cur.chars().filter(|c| *c == '<').count();
            let closes = cur.chars().filter(|c| *c == '>').count();
            if closes > opens {
                cur.pop();
                continue;
            }
        }
        break;
    }

    let (reg, suf) = cur.split_once('/')?;
    if !reg.starts_with("10.")
        || reg.len() < 7
        || !reg[3..].chars().all(|c| c.is_ascii_digit() || c == '.')
        || suf.len() < 2
        || suf.ends_with('-')
    {
        return None;
    }
    Some(cur.to_ascii_lowercase())
}

/// Extracts the underlying arXiv identifier from an arXiv DataCite DOI (`10.48550/arXiv.2301.12345`).
pub fn extract_arxiv_from_doi(doi: &str) -> Option<String> {
    let norm = normalize_doi(doi)?;
    let rest = norm.strip_prefix("10.48550/arxiv.")?;
    normalize_arxiv_base_id(rest)
}

/// Strips inline XML/HTML/JATS tags (`<i>`, `<scp>`, `<jats:p>`, ...) and decodes basic HTML entities,
/// collapsing whitespace cleanly.
pub fn clean_metadata_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => {
                in_tag = false;
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parses an author display name (`"Family, Given"` or `"Given M. Family"`, optionally with
/// trailing DBLP 4-digit disambiguation numbers like `"Yang Liu 0001"`) into `BibliographicAuthor`.
pub fn split_full_author_name(raw: &str, role: &str) -> Option<BibliographicAuthor> {
    let cleaned = clean_metadata_text(raw);
    if cleaned.is_empty() {
        return None;
    }

    if let Some((fam_raw, giv_raw)) = cleaned.split_once(',') {
        let fam = strip_dblp_homonym_suffix(fam_raw.trim());
        let giv = strip_dblp_homonym_suffix(giv_raw.trim());
        if fam.is_empty() && giv.is_empty() {
            return None;
        }
        return Some(BibliographicAuthor {
            family_name: fam,
            given_name: giv,
            role: role.to_string(),
        });
    }

    let stripped = strip_dblp_homonym_suffix(&cleaned);
    let parts: Vec<&str> = stripped.split_whitespace().collect();
    match parts.as_slice() {
        [] => None,
        [single] => Some(BibliographicAuthor {
            family_name: (*single).to_string(),
            given_name: String::new(),
            role: role.to_string(),
        }),
        many => Some(BibliographicAuthor {
            family_name: (*many.last().unwrap_or(&"")).to_string(),
            given_name: many[..many.len() - 1].join(" "),
            role: role.to_string(),
        }),
    }
}

fn strip_dblp_homonym_suffix(s: &str) -> String {
    let mut parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() >= 2 {
        if let Some(last) = parts.last() {
            if last.len() == 4 && last.chars().all(|c| c.is_ascii_digit()) {
                parts.pop();
            }
        }
    }
    parts.join(" ")
}

/// Normalizes an arXiv identifier to its lowercase versionless base ID
/// (e.g. `"arXiv:2301.12345v2"` -> `Some("2301.12345")`, `"math.NA/0501234v1"` -> `Some("math.na/0501234")`).
pub fn normalize_arxiv_base_id(raw: &str) -> Option<String> {
    let mut s = raw.trim();
    for prefix in [
        "https://arxiv.org/abs/",
        "http://arxiv.org/abs/",
        "https://export.arxiv.org/abs/",
        "http://export.arxiv.org/abs/",
        "arxiv:",
        "arXiv:",
        "ARXIV:",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim();
        }
    }
    let cleaned = s
        .trim_end_matches(['.', ',', ';', ')', ']', '/', '>'])
        .split(['?', '#'])
        .next()?
        .trim();
    if cleaned.is_empty() {
        return None;
    }
    let base = if let Some((b, ver)) = cleaned.rsplit_once(['v', 'V']) {
        if !b.is_empty() && !ver.is_empty() && ver.chars().all(|c| c.is_ascii_digit()) {
            b
        } else {
            cleaned
        }
    } else {
        cleaned
    };
    if base.is_empty() {
        None
    } else {
        Some(base.to_ascii_lowercase())
    }
}

/// Extracts a clean alphabetic family name from a heuristic author string
/// (`"Franco Brezzi*"` -> `"Brezzi"`, `"Brezzi, F."` -> `"Brezzi"`, `"De Giorgi, Ennio"` -> `"De Giorgi"`).
pub fn clean_author_surname(raw: &str) -> Option<String> {
    let cleaned = clean_metadata_text(raw);
    if cleaned.is_empty() {
        return None;
    }
    if let Some((fam, _)) = cleaned.split_once(',') {
        let words: Vec<String> = fam
            .split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphabetic() || *c == '-' || *c == '\'')
                    .collect::<String>()
            })
            .map(|w| w.trim_matches(['-', '\'']).to_string())
            .filter(|w| !w.is_empty())
            .collect();
        let joined = words.join(" ");
        if joined.chars().filter(|c| c.is_alphabetic()).count() >= 2 {
            return Some(joined);
        }
        return None;
    }

    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let candidate = words.last().copied().unwrap_or("");
    let trimmed: String = candidate
        .chars()
        .filter(|c| c.is_alphabetic() || *c == '-' || *c == '\'')
        .collect();
    let trimmed = trimmed.trim_matches(['-', '\'']);
    if trimmed.chars().count() >= 2 {
        if words.len() >= 2 {
            let prev_raw = words[words.len() - 2];
            let prev_clean: String = prev_raw
                .chars()
                .filter(|c| c.is_alphabetic() || *c == '\'')
                .collect();
            if matches!(
                prev_clean.to_ascii_lowercase().as_str(),
                "de" | "del" | "della" | "di" | "da" | "du" | "van" | "von" | "der" | "den" | "la" | "le"
            ) {
                return Some(format!("{prev_clean} {trimmed}"));
            }
        }
        Some(trimmed.to_string())
    } else {
        None
    }
}
