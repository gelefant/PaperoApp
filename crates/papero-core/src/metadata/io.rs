use crate::bibtex::{format_mrlookup_bibtex, parse_bibtex_entries, BibtexExportOptions};
use crate::error::{CoreError, Result};
use crate::metadata::models::{
    normalize_doi, normalize_pages, BibliographicAuthor, BibliographicRecord, DocumentMetadataView,
    WorkType,
};
use serde_json::Value;

pub fn metadata_view_to_record(doc: &DocumentMetadataView) -> BibliographicRecord {
    BibliographicRecord {
        work_type: WorkType::from_str_lossy(&doc.work_type),
        title: doc.title.clone(),
        authors: doc.authors.clone(),
        journal_abbrev: doc.journal_abbrev.clone(),
        journal_full: doc.journal_full.clone(),
        booktitle: doc.booktitle.clone(),
        series: doc.series.clone(),
        volume: doc.volume.clone(),
        issue_number: doc.issue_number.clone(),
        year: doc.year,
        pages: doc.pages.clone(),
        edition: doc.edition.clone(),
        note: None,
        publisher: doc.publisher.clone(),
        address: doc.address.clone(),
        isbn: doc.isbn.clone(),
        issn: doc.issn.clone(),
        coden: None,
        doi: doc.doi.clone(),
        arxiv_id: doc.arxiv_id.clone(),
        arxiv_version: doc.arxiv_version.clone(),
        mr_number: doc.mr_number.clone(),
        mr_number_display: doc.mr_number_display.clone(),
        mr_reviewer: doc.mr_reviewer.clone(),
        zbl_number: doc.zbl_number.clone(),
        url: doc.url.clone(),
        msc_primary: doc.msc_primary.clone(),
        msc_secondary: doc.msc_secondary.clone(),
        abstract_text: doc.abstract_text.clone(),
        raw_mrlookup_bibtex: doc.raw_mrlookup_bibtex.clone(),
        source: doc
            .metadata_source
            .clone()
            .unwrap_or_else(|| "manual".to_string()),
        license_note: doc.metadata_license_note.clone(),
    }
}

/// Exports multiple documents as concatenated MRLookup-style BibTeX entries.
pub fn export_documents_bibtex(docs: &[DocumentMetadataView], opts: &BibtexExportOptions) -> String {
    docs.iter()
        .map(|d| format_mrlookup_bibtex(&metadata_view_to_record(d), opts))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Exports multiple documents as RFC 4180 CSV.
pub fn export_documents_csv(docs: &[DocumentMetadataView]) -> String {
    let mut out = String::from(
        "document_id,work_type,title,authors,year,journal,volume,issue,pages,doi,arxiv_id,mr_number,zbl_number,isbn,msc_primary,source\n",
    );
    for d in docs {
        let authors = d
            .authors
            .iter()
            .filter(|a| a.role == "author")
            .map(|a| a.format_bib())
            .collect::<Vec<_>>()
            .join("; ");
        let row = [
            d.document_id.to_string(),
            d.work_type.clone(),
            d.title.clone(),
            authors,
            d.year.map(|y| y.to_string()).unwrap_or_default(),
            d.journal_full
                .clone()
                .or_else(|| d.journal_abbrev.clone())
                .unwrap_or_default(),
            d.volume.clone().unwrap_or_default(),
            d.issue_number.clone().unwrap_or_default(),
            d.pages.clone().unwrap_or_default(),
            d.doi.clone().unwrap_or_default(),
            d.arxiv_id.clone().unwrap_or_default(),
            d.mr_number.clone().unwrap_or_default(),
            d.zbl_number.clone().unwrap_or_default(),
            d.isbn.clone().unwrap_or_default(),
            d.msc_primary.clone().unwrap_or_default(),
            d.metadata_source.clone().unwrap_or_default(),
        ];
        let quoted: Vec<String> = row.iter().map(|c| csv_escape(c)).collect();
        out.push_str(&quoted.join(","));
        out.push('\n');
    }
    out
}

fn csv_escape(cell: &str) -> String {
    if cell.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", cell.replace('"', "\"\""))
    } else {
        cell.to_string()
    }
}

/// Imports BibTeX `.bib` file contents into `BibliographicRecord` entries.
pub fn import_bibtex(content: &str) -> Vec<BibliographicRecord> {
    parse_bibtex_entries(content)
        .into_iter()
        .map(|e| e.record)
        .collect()
}

/// Imports Zotero CSL-JSON array/object export into `BibliographicRecord` entries.
pub fn import_zotero_csl_json(content: &str) -> Result<Vec<BibliographicRecord>> {
    let val: Value = serde_json::from_str(content)
        .map_err(|e| CoreError::Config(format!("invalid Zotero CSL-JSON: {e}")))?;
    let arr = match val {
        Value::Array(items) => items,
        Value::Object(_) => vec![val],
        _ => return Err(CoreError::Config("expected CSL-JSON array or object".into())),
    };

    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(title) = item
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
        else {
            continue;
        };
        let work_type = item
            .get("type")
            .and_then(Value::as_str)
            .map(WorkType::from_str_lossy)
            .unwrap_or(WorkType::Article);

        let mut authors = Vec::new();
        for a in item.get("author").and_then(Value::as_array).into_iter().flatten() {
            let fam = a.get("family").and_then(Value::as_str).unwrap_or("").trim();
            let giv = a.get("given").and_then(Value::as_str).unwrap_or("").trim();
            if !fam.is_empty() || !giv.is_empty() {
                authors.push(BibliographicAuthor {
                    family_name: fam.to_string(),
                    given_name: giv.to_string(),
                    role: "author".to_string(),
                });
            }
        }
        for e in item.get("editor").and_then(Value::as_array).into_iter().flatten() {
            let fam = e.get("family").and_then(Value::as_str).unwrap_or("").trim();
            let giv = e.get("given").and_then(Value::as_str).unwrap_or("").trim();
            if !fam.is_empty() || !giv.is_empty() {
                authors.push(BibliographicAuthor {
                    family_name: fam.to_string(),
                    given_name: giv.to_string(),
                    role: "editor".to_string(),
                });
            }
        }

        let year = item
            .get("issued")
            .and_then(|d| d.get("date-parts"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_array)
            .and_then(|p| p.first())
            .and_then(|y| y.as_i64().or_else(|| y.as_str()?.parse::<i64>().ok()));

        let container = item
            .get("container-title")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let (journal_full, booktitle) = match work_type {
            WorkType::Incollection | WorkType::Inproceedings => (None, container),
            _ => (container, None),
        };

        out.push(BibliographicRecord {
            work_type,
            title,
            authors,
            journal_abbrev: item
                .get("container-title-short")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            journal_full,
            booktitle,
            series: item
                .get("collection-title")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            volume: item.get("volume").and_then(json_str_or_int),
            issue_number: item.get("issue").and_then(json_str_or_int),
            year,
            pages: item
                .get("page")
                .and_then(Value::as_str)
                .map(normalize_pages),
            edition: item.get("edition").and_then(json_str_or_int),
            publisher: item
                .get("publisher")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            address: item
                .get("publisher-place")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            isbn: item
                .get("ISBN")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            issn: item
                .get("ISSN")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            doi: item.get("DOI").and_then(Value::as_str).and_then(normalize_doi),
            url: item.get("URL").and_then(Value::as_str).map(ToOwned::to_owned),
            abstract_text: item
                .get("abstract")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            source: "zotero_csl".to_string(),
            ..Default::default()
        });
    }

    Ok(out)
}

fn json_str_or_int(v: &Value) -> Option<String> {
    v.as_str()
        .map(ToOwned::to_owned)
        .or_else(|| v.as_i64().map(|n| n.to_string()))
}
