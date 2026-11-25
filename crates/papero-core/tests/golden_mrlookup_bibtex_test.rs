use papero_core::bibtex::{format_mrlookup_bibtex, parse_bibtex_entries, BibtexExportOptions};
use papero_core::db::repos::docs::create_heuristic_document;
use papero_core::db::Database;
use papero_core::metadata::resolver::{apply_manual_metadata_edit, apply_record_respecting_locks, ManualMetadataEdit};
use papero_core::metadata::models::{BibliographicAuthor, BibliographicRecord, WorkType};
use std::fs;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn golden_mrlookup_bibtex_matches_verbatim_across_at_least_20_entries() {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden_bibtex");
    let mut files: Vec<PathBuf> = fs::read_dir(&fixture_dir)
        .expect("read golden_bibtex dir")
        .filter_map(|e| e.ok().map(|x| x.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("bib"))
        .collect();
    files.sort();

    assert!(
        files.len() >= 20,
        "expected >= 20 golden MRLookup entries, found {}",
        files.len()
    );

    let mut saw_article = false;
    let mut saw_book = false;
    let mut saw_incollection = false;
    let mut saw_inproceedings = false;

    for path in &files {
        let verbatim = fs::read_to_string(path).expect("read .bib fixture");
        let verbatim_trimmed = verbatim.trim();
        let parsed = parse_bibtex_entries(verbatim_trimmed);
        assert_eq!(
            parsed.len(),
            1,
            "fixture {} should parse cleanly as 1 entry",
            path.display()
        );
        let entry = &parsed[0];
        assert!(
            entry.record.mr_number.is_some(),
            "fixture {} missing MRNUMBER",
            path.display()
        );

        match entry.record.work_type {
            WorkType::Article => saw_article = true,
            WorkType::Book => saw_book = true,
            WorkType::Incollection => saw_incollection = true,
            WorkType::Inproceedings => saw_inproceedings = true,
            _ => {}
        }

        // Direct MR path MUST reproduce verbatim MRLookup BibTeX character-for-character
        let formatted_direct = format_mrlookup_bibtex(&entry.record, &BibtexExportOptions::default());
        assert_eq!(
            formatted_direct,
            verbatim_trimmed,
            "direct MR output mismatch in {}",
            path.display()
        );

        // Reconstructed path (without raw_mrlookup_bibtex) MUST follow exact MRLookup structure:
        // single space `@type {key,`, 10-char right-aligned uppercase keys, `--` pages, `}`
        let mut reconstructed_rec = entry.record.clone();
        reconstructed_rec.raw_mrlookup_bibtex = None;
        let reconstructed = format_mrlookup_bibtex(
            &reconstructed_rec,
            &BibtexExportOptions {
                wrap_lines: true,
                include_source_comment: false,
                citation_key_override: Some(entry.key.clone()),
                ..Default::default()
            },
        );
        let header_prefix = format!("@{} {{{},", entry.entry_type, entry.key);
        assert!(
            reconstructed.starts_with(&header_prefix),
            "bad reconstructed header in {}: {}",
            path.display(),
            reconstructed
        );
        for line in reconstructed.lines().skip(1) {
            if line == "}" || line.starts_with("              ") {
                continue;
            }
            let eq_idx = line.find(" = {").expect("10-char aligned field separator");
            assert_eq!(
                eq_idx, 10,
                "field alignment != 10 chars in {}: {line}",
                path.display()
            );
        }
        assert!(reconstructed.ends_with("\n}"));
    }

    assert!(saw_article, "missing @article in golden suite");
    assert!(saw_book, "missing @book in golden suite");
    assert!(saw_incollection, "missing @incollection in golden suite");
    assert!(saw_inproceedings, "missing @inproceedings in golden suite");
}

#[test]
fn manual_locked_fields_are_never_overwritten_by_autofetch() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("papero.db"), 2).unwrap();

    let doc_id = {
        let w = db.writer();
        create_heuristic_document(&w, "Original Heuristic").unwrap()
    };

    apply_manual_metadata_edit(
        &db,
        &ManualMetadataEdit {
            document_id: doc_id,
            work_type: "article".into(),
            title: "My Custom Locked Title".into(),
            authors: vec![BibliographicAuthor {
                family_name: "Mumford".into(),
                given_name: "David".into(),
                role: "author".into(),
            }],
            journal_abbrev: None,
            journal_full: None,
            booktitle: None,
            series: None,
            volume: None,
            issue_number: None,
            year: Some(1989),
            pages: None,
            edition: None,
            publisher: None,
            address: None,
            isbn: None,
            issn: None,
            doi: None,
            arxiv_id: None,
            mr_number: None,
            mr_reviewer: None,
            zbl_number: None,
            url: None,
            msc_primary: Some("49F20".into()),
            msc_secondary: vec![],
            abstract_text: None,
            citation_key_override: None,
            exclude_from_cloud_llm: false,
            tags: vec!["variational".into()],
            edited_fields: vec!["title".into(), "year".into(), "msc_primary".into()],
        },
    )
    .unwrap();

    let remote = BibliographicRecord {
        work_type: WorkType::Article,
        title: "Remote Overwrite Attempt".into(),
        year: Some(2025),
        doi: Some("10.1002/cpa.3160420503".into()),
        journal_abbrev: Some("Comm. Pure Appl. Math.".into()),
        msc_primary: Some("00A00".into()),
        source: "zbmath".into(),
        ..Default::default()
    };

    {
        let w = db.writer();
        apply_record_respecting_locks(&w, doc_id, &remote, false).unwrap();
    }

    let r = db.reader();
    let (title, year, doi, msc): (String, Option<i64>, Option<String>, Option<String>) = r
        .query_row(
            "SELECT title, year, doi, msc_primary FROM documents WHERE id = ?1",
            [doc_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();

    assert_eq!(title, "My Custom Locked Title");
    assert_eq!(year, Some(1989));
    assert_eq!(msc.as_deref(), Some("49F20"));
    assert_eq!(doi.as_deref(), Some("10.1002/cpa.3160420503"));
}
