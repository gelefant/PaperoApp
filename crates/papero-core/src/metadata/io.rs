use crate::bibtex::{
    compute_citation_key, disambiguate_citation_keys, format_mrlookup_bibtex, parse_bibtex_entries,
    BibtexExportOptions,
};
use crate::error::{CoreError, Result};
use crate::metadata::models::{
    normalize_doi, normalize_pages, BibliographicAuthor, BibliographicRecord, DocumentMetadataView,
    WorkType,
};
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

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

/// Exports multiple documents as concatenated BibTeX/BibLaTeX entries with automatic
/// anti-collision key disambiguation (`a`, `b`, `c`, ...).
pub fn export_documents_bibtex(docs: &[DocumentMetadataView], opts: &BibtexExportOptions) -> String {
    if docs.is_empty() {
        return String::new();
    }
    let records: Vec<BibliographicRecord> = docs.iter().map(metadata_view_to_record).collect();
    let base_keys: Vec<String> = records
        .iter()
        .map(|r| compute_citation_key(r, opts.key_pattern))
        .collect();
    let unique_keys = disambiguate_citation_keys(&base_keys);

    records
        .iter()
        .zip(base_keys.iter())
        .zip(unique_keys.iter())
        .map(|((rec, base_k), uniq_k)| {
            let mut item_opts = opts.clone();
            if uniq_k != base_k {
                item_opts.citation_key_override = Some(uniq_k.clone());
            }
            format_mrlookup_bibtex(rec, &item_opts)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Verifies `dest` is strictly outside all configured read-only `source_roots` directories,
/// then writes `content` atomically via a temporary sibling file (Windows sharing-safe).
pub fn write_bibtex_file_atomically(
    dest: &Path,
    content: &str,
    source_root_paths: &[String],
) -> Result<()> {
    if dest.as_os_str().is_empty() {
        return Err(CoreError::Config("BibTeX destination path is empty".into()));
    }

    let check_dir = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canon_parent = canonicalize_best_effort(check_dir);
    for root_raw in source_root_paths {
        let canon_root = canonicalize_best_effort(Path::new(root_raw));
        if path_is_inside(&canon_parent, &canon_root) {
            return Err(CoreError::Config(format!(
                "Strictly read-only guard: cannot write .bib file inside source folder '{}'",
                root_raw
            )));
        }
    }

    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let base_name = dest
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("papero.bib");
    let tmp_path = dest.with_file_name(format!(".{base_name}.{}.tmp", uuid::Uuid::new_v4()));

    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(content.as_bytes())?;
        if !content.is_empty() && !content.ends_with('\n') {
            f.write_all(b"\n")?;
        }
        f.sync_all()?;
    }

    // Rename with Windows-safe overwrite & brief retry if an editor/TeX compiler is reading the .bib
    let mut last_err: Option<std::io::Error> = None;
    for attempt in 0..10 {
        match std::fs::rename(&tmp_path, dest) {
            Ok(()) => return Ok(()),
            Err(e) => {
                #[cfg(target_os = "windows")]
                if dest.exists() {
                    let _ = std::fs::remove_file(dest);
                    if std::fs::rename(&tmp_path, dest).is_ok() {
                        return Ok(());
                    }
                }
                last_err = Some(e);
                if attempt < 9 {
                    std::thread::sleep(Duration::from_millis(30));
                }
            }
        }
    }
    let _ = std::fs::remove_file(&tmp_path);
    Err(CoreError::Io(
        last_err.unwrap_or_else(|| std::io::Error::other("atomic bibtex write failed")),
    ))
}

fn canonicalize_best_effort(path: &Path) -> std::path::PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    if let Ok(canon) = std::fs::canonicalize(&abs) {
        return canon;
    }
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = abs.as_path();
    while let Some(parent) = cur.parent() {
        if let Some(name) = cur.file_name() {
            tail.push(name.to_os_string());
        }
        if parent.exists() {
            let mut base = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            for seg in tail.into_iter().rev() {
                base.push(seg);
            }
            return base;
        }
        cur = parent;
    }
    abs
}

#[cfg(any(target_os = "windows", test))]
fn normalize_windows_path_for_prefix(raw: &str) -> String {
    let stripped = crate::db::repos::roots::strip_windows_verbatim_prefix(raw);
    stripped
        .replace('/', "\\")
        .to_ascii_lowercase()
        .trim_end_matches('\\')
        .to_string()
}

#[cfg(any(target_os = "windows", test))]
fn windows_path_is_inside_str(candidate: &str, root: &str) -> bool {
    let c = normalize_windows_path_for_prefix(candidate);
    let r = normalize_windows_path_for_prefix(root);
    if r.is_empty() {
        return false;
    }
    c == r || c.starts_with(&format!("{r}\\"))
}

fn path_is_inside(candidate: &Path, root: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_path_is_inside_str(&candidate.to_string_lossy(), &root.to_string_lossy())
    }
    #[cfg(not(target_os = "windows"))]
    {
        candidate.starts_with(root)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bibtex::BibtexKeyPattern;

    #[test]
    fn write_bibtex_atomically_blocks_source_roots_and_writes_outside() {
        let tmp = tempfile::tempdir().unwrap();
        let src_root = tmp.path().join("readonly_papers");
        let external_dir = tmp.path().join("latex_project");
        std::fs::create_dir_all(&src_root).unwrap();
        std::fs::create_dir_all(&external_dir).unwrap();

        let roots = vec![src_root.to_string_lossy().into_owned()];
        let forbidden = src_root.join("refs.bib");
        assert!(write_bibtex_file_atomically(&forbidden, "@misc{a,}", &roots).is_err());
        assert!(!forbidden.exists());

        // Non-existent nested subdirectory inside a source root must also be rejected without creating dirs.
        let forbidden_nested = src_root.join("new_sub").join("refs.bib");
        assert!(write_bibtex_file_atomically(&forbidden_nested, "@misc{a,}", &roots).is_err());
        assert!(!src_root.join("new_sub").exists());

        // Windows verbatim `\\?\` / mixed-slash / case-insensitive prefix checks.
        assert!(windows_path_is_inside_str(
            r"C:\Users\Math\Papers\sub",
            r"\\?\c:\users\math\papers"
        ));
        assert!(windows_path_is_inside_str(
            "c:/Users/Math/Papers/sub",
            r"\\?\C:\Users\Math\Papers\"
        ));
        assert!(!windows_path_is_inside_str(
            r"C:\Users\Math\Papers2\refs",
            r"\\?\C:\Users\Math\Papers"
        ));

        let allowed = external_dir.join("refs.bib");
        write_bibtex_file_atomically(&allowed, "@misc{a,}", &roots).unwrap();
        assert_eq!(std::fs::read_to_string(&allowed).unwrap(), "@misc{a,}\n");
    }

    #[test]
    fn export_documents_bibtex_disambiguates_colliding_keys() {
        let mk = |id: i64, title: &str| DocumentMetadataView {
            document_id: id,
            work_type: "article".into(),
            title: title.into(),
            authors: vec![BibliographicAuthor {
                family_name: "Erdős".into(),
                given_name: "Paul".into(),
                role: "author".into(),
            }],
            journal_abbrev: None,
            journal_full: None,
            booktitle: None,
            series: None,
            volume: None,
            issue_number: None,
            year: Some(1975),
            pages: None,
            edition: None,
            publisher: None,
            address: None,
            isbn: None,
            issn: None,
            doi: None,
            arxiv_id: None,
            arxiv_version: None,
            mr_number: None,
            mr_number_display: None,
            mr_reviewer: None,
            zbl_number: None,
            url: None,
            msc_primary: None,
            msc_primary_status: "none".into(),
            msc_secondary: vec![],
            abstract_text: None,
            raw_mrlookup_bibtex: None,
            metadata_source: Some("manual".into()),
            metadata_license_note: None,
            manual_locked_fields: vec![],
            metadata_verified: false,
            exclude_from_cloud_llm: false,
            reading_status: "unread".into(),
            is_starred: false,
            collection_ids: vec![],
            tags: vec![],
            pending_candidates: vec![],
        };
        let docs = vec![mk(1, "First Paper"), mk(2, "Second Paper")];
        let out = export_documents_bibtex(
            &docs,
            &BibtexExportOptions {
                key_pattern: BibtexKeyPattern::AuthorYear,
                include_source_comment: false,
                ..Default::default()
            },
        );
        assert!(out.contains("@article {Erdos1975a,"));
        assert!(out.contains("@article {Erdos1975b,"));
    }
}
