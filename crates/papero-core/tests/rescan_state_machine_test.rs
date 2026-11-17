use papero_core::config::AppConfig;
use papero_core::db::repos::roots::{insert_source_root, NewSourceRoot};
use papero_core::db::Database;
use papero_core::extract::djvu::extract_djvu;
use papero_core::fs::roots::new_root_id;
use papero_core::fs::scanner::rescan_sources;
use papero_core::index::bm25::Bm25Index;
use papero_core::index::worker::run_extract_bm25_queue;
use rusqlite::params;
use std::fs;
use std::process::Command;

#[test]
fn rescan_handles_all_seven_states_and_manual_archive_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("papero.db");
    let tantivy_dir = tmp.path().join("tantivy");
    let root_dir = tmp.path().join("library");
    let offline_dir = tmp.path().join("offline_drive");
    fs::create_dir_all(&root_dir).unwrap();
    fs::create_dir_all(&offline_dir).unwrap();

    let db = Database::open(&db_path, 2).unwrap();
    let bm25 = Bm25Index::open_or_create(&tantivy_dir).unwrap();

    let root_id = new_root_id();
    let offline_root_id = new_root_id();
    {
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: root_id.clone(),
                label: "Main".into(),
                root_path: root_dir.to_string_lossy().into_owned(),
                is_network_or_cloud: false,
            },
        )
        .unwrap();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: offline_root_id.clone(),
                label: "External".into(),
                root_path: offline_dir.to_string_lossy().into_owned(),
                is_network_or_cloud: false,
            },
        )
        .unwrap();
    }

    // Seed initial files
    let file_a = root_dir.join("paper_a.pdf");
    let file_b = root_dir.join("paper_b.pdf");
    let file_off = offline_dir.join("book_off.pdf");
    fs::write(&file_a, b"%PDF-1.4\nAlpha paper content unique A\n").unwrap();
    fs::write(&file_b, b"%PDF-1.4\nBeta paper content unique B\n").unwrap();
    fs::write(&file_off, b"%PDF-1.4\nOffline book content\n").unwrap();

    // State 1: New files
    let s1 = rescan_sources(&db, Some(&bm25), None, None).unwrap();
    assert_eq!(s1.cnt_new, 3);
    assert_eq!(s1.cnt_unchanged, 0);

    // Manually edit title + add tag + lock field on paper_a so archive can be tested
    let (doc_a_id, doc_b_id) = {
        let w = db.writer();
        let doc_a: i64 = w
            .query_row(
                "SELECT document_id FROM files WHERE rel_path = 'paper_a.pdf'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let doc_b: i64 = w
            .query_row(
                "SELECT document_id FROM files WHERE rel_path = 'paper_b.pdf'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        w.execute(
            "UPDATE documents SET title = 'Manual Title A', manual_locked_fields = '[\"title\"]', msc_primary = '11A05', msc_primary_status = 'confirmed' WHERE id = ?1",
            params![doc_a],
        )
        .unwrap();
        w.execute("INSERT INTO tags (name) VALUES ('favorite-math')", [])
            .unwrap();
        let tag_id = w.last_insert_rowid();
        w.execute(
            "INSERT INTO document_tags (document_id, tag_id) VALUES (?1, ?2)",
            params![doc_a, tag_id],
        )
        .unwrap();
        (doc_a, doc_b)
    };

    // State 2: Unchanged rescan
    let s2 = rescan_sources(&db, Some(&bm25), None, None).unwrap();
    assert_eq!(s2.cnt_unchanged, 3);
    assert_eq!(s2.cnt_new, 0);

    // State 3, 4, 5, 6, 7 simultaneously:
    // - paper_a.pdf -> REMOVED (should archive manual metadata & tag!)
    // - paper_b.pdf -> MOVED/RENAMED to subdir/paper_b_renamed.pdf (same document_id, zero reindex!)
    // - subdir/paper_b_copy.pdf -> IDENTICAL COPY of paper_b (Level 1 duplicate!)
    // - paper_c.pdf -> NEW
    // - offline_dir -> removed from disk -> OFFLINE ROOT PRESERVED!
    let bytes_a = fs::read(&file_a).unwrap();
    fs::remove_file(&file_a).unwrap();

    fs::create_dir_all(root_dir.join("subdir")).unwrap();
    let moved_b = root_dir.join("subdir/paper_b_renamed.pdf");
    let copy_b = root_dir.join("subdir/paper_b_copy.pdf");
    fs::rename(&file_b, &moved_b).unwrap();
    fs::copy(&moved_b, &copy_b).unwrap();

    let file_c = root_dir.join("paper_c.pdf");
    fs::write(&file_c, b"%PDF-1.4\nGamma paper content unique C\n").unwrap();

    fs::remove_dir_all(&offline_dir).unwrap();

    let s3 = rescan_sources(&db, Some(&bm25), None, None).unwrap();
    assert_eq!(s3.cnt_removed, 1);
    assert_eq!(s3.cnt_moved_renamed, 1);
    assert_eq!(s3.cnt_identical_copy, 1);
    assert_eq!(s3.cnt_new, 1);
    assert_eq!(s3.cnt_dup_groups_new, 1);

    {
        let r = db.reader();
        // Offline file MUST still exist in catalog, marked unavailable
        let off_avail: i64 = r
            .query_row(
                "SELECT is_available FROM files WHERE rel_path = 'book_off.pdf'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(off_avail, 0);

        // Moved & copied files share original doc_b_id
        let b_file_count: i64 = r
            .query_row(
                "SELECT COUNT(*) FROM files WHERE document_id = ?1",
                params![doc_b_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(b_file_count, 2);

        // Removed doc_a should be gone from documents, archived in manual_metadata_archive
        let doc_a_exists: i64 = r
            .query_row(
                "SELECT COUNT(*) FROM documents WHERE id = ?1",
                params![doc_a_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(doc_a_exists, 0);
    }

    // Reintroduce paper_a under a new name -> manual metadata & tags restored automatically!
    // Also MODIFY paper_c.pdf -> triggers Modified state & content_modified_notice = 1.
    let file_a_reborn = root_dir.join("reborn_a.pdf");
    fs::write(&file_a_reborn, &bytes_a).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(&file_c, b"%PDF-1.4\nGamma paper REPLACED content!\n").unwrap();

    let s4 = rescan_sources(&db, Some(&bm25), None, None).unwrap();
    assert_eq!(s4.cnt_new, 1);
    assert_eq!(s4.cnt_modified, 1);

    {
        let r = db.reader();
        let (restored_title, restored_msc): (String, Option<String>) = r
            .query_row(
                "SELECT d.title, d.msc_primary FROM documents d
                 JOIN files f ON f.document_id = d.id
                 WHERE f.rel_path = 'reborn_a.pdf'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(restored_title, "Manual Title A");
        assert_eq!(restored_msc.as_deref(), Some("11A05"));

        let restored_tag: String = r
            .query_row(
                "SELECT t.name FROM tags t
                 JOIN document_tags dt ON dt.tag_id = t.id
                 JOIN files f ON f.document_id = dt.document_id
                 WHERE f.rel_path = 'reborn_a.pdf'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(restored_tag, "favorite-math");

        let mod_notice: i64 = r
            .query_row(
                "SELECT d.content_modified_notice FROM documents d
                 JOIN files f ON f.document_id = d.id
                 WHERE f.rel_path = 'paper_c.pdf'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mod_notice, 1);
    }
}

#[test]
fn end_to_end_djvu_extraction_chunking_and_bm25_search() {
    if Command::new("djvused").arg("--help").output().is_err()
        || Command::new("cjb2").arg("--help").output().is_err()
    {
        // Skip if DjVuLibre CLI tools are absent in a minimal CI environment
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let root_dir = tmp.path().join("djvu_lib");
    fs::create_dir_all(&root_dir).unwrap();

    // Generate a real DjVu file with cjb2 + embedded text layer & outline via djvused
    let pbm_path = tmp.path().join("page1.pbm");
    let mut pbm = String::from("P1\n200 400\n");
    for _ in 0..400 {
        pbm.push_str(&"0 ".repeat(200));
        pbm.push('\n');
    }
    fs::write(&pbm_path, pbm).unwrap();

    let djvu_path = root_dir.join("poincare_erdos.djvu");
    let status = Command::new("cjb2")
        .arg(&pbm_path)
        .arg(&djvu_path)
        .status()
        .unwrap();
    assert!(status.success());

    let script = concat!(
        "select 1\n",
        "set-txt\n",
        "(page 0 0 200 400\n",
        " (line 20 320 180 350 \"Sur les groupes de Poincaré et les graphes d'Erdős.\"))\n",
        ".\n",
        "set-outline\n",
        "(bookmarks (\"Introduction\" \"#1\"))\n",
        ".\n",
    );
    let mut child = Command::new("djvused")
        .arg("-s")
        .arg(&djvu_path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
    }
    assert!(child.wait().unwrap().success());

    // Verify direct extraction
    let extracted = extract_djvu(&djvu_path, None).unwrap();
    assert_eq!(extracted.page_count, 1);
    assert!(extracted.has_text_layer);
    assert_eq!(extracted.outline.len(), 1);
    assert_eq!(extracted.outline[0].title, "Introduction");

    // Verify full pipeline: rescan -> worker extract_bm25 -> BM25 search without diacritics
    let db = Database::open(&tmp.path().join("papero.db"), 2).unwrap();
    let bm25 = Bm25Index::open_or_create(&tmp.path().join("tantivy")).unwrap();
    let root_id = new_root_id();
    {
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: root_id.clone(),
                label: "DjVu".into(),
                root_path: root_dir.to_string_lossy().into_owned(),
                is_network_or_cloud: false,
            },
        )
        .unwrap();
    }

    let scan = rescan_sources(&db, Some(&bm25), None, None).unwrap();
    assert_eq!(scan.cnt_new, 1);

    let cfg = AppConfig::default();
    let report = run_extract_bm25_queue(&db, &bm25, &cfg, None).unwrap();
    assert_eq!(report.succeeded, 1);
    assert_eq!(report.passages_indexed, 1);

    let hits = bm25.search("Poincare Erdos", None, 10).unwrap();
    assert_eq!(hits.len(), 1);

    let passage = papero_core::db::repos::docs::get_passage_by_id(&db.reader(), hits[0].passage_id)
        .unwrap()
        .unwrap();
    assert_eq!(passage.page_ord, 1);
    assert_eq!(passage.bboxes.len(), 1);
    // ymin=320, ymax=350 on H=400 -> top-left y = (400-350)/400 = 0.125
    assert!((passage.bboxes[0].y - 0.125).abs() < 1e-4);
}

#[test]
fn rescan_with_changed_mtime_and_identical_blake3_keeps_passages_and_marks_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let db = Database::open(&tmp.path().join("papero.db"), 2).unwrap();
    let bm25 = Bm25Index::open_or_create(&tmp.path().join("tantivy")).unwrap();
    let root_dir = tmp.path().join("drive");
    fs::create_dir_all(&root_dir).unwrap();

    let root_id = new_root_id();
    {
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: root_id.clone(),
                label: "Drive".into(),
                root_path: root_dir.to_string_lossy().into_owned(),
                is_network_or_cloud: true,
            },
        )
        .unwrap();
    }

    let pdf_path = root_dir.join("synced.pdf");
    let payload = b"%PDF-1.4\nCross-OS identical bytes\n";
    fs::write(&pdf_path, payload).unwrap();

    let s1 = rescan_sources(&db, Some(&bm25), None, None).unwrap();
    assert_eq!(s1.cnt_new, 1);

    // Seed a passage and mark embedded_light=1, then simulate cross-OS mtime_ns shift in DB
    {
        let w = db.writer();
        let (file_id, doc_id): (i64, i64) = w
            .query_row(
                "SELECT id, document_id FROM files WHERE rel_path = 'synced.pdf'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        w.execute(
            "UPDATE files SET mtime_ns = mtime_ns - 999999, index_state = 'ready', embedded_light = 1 WHERE id = ?1",
            params![file_id],
        )
        .unwrap();
        w.execute(
            "INSERT INTO passages (document_id, file_id, page_ord, chunk_index, is_alt_formula, token_count, text_content, bboxes_blob)
             VALUES (?1, ?2, 1, 0, 0, 5, 'Preserved passage content', X'')",
            params![doc_id, file_id],
        )
        .unwrap();
    }

    let s2 = rescan_sources(&db, Some(&bm25), None, None).unwrap();
    assert_eq!(s2.cnt_unchanged, 1);
    assert_eq!(s2.cnt_modified, 0);

    let r = db.reader();
    let (passage_cnt, notice, embedded_light): (i64, i64, i64) = r
        .query_row(
            "SELECT (SELECT COUNT(*) FROM passages), d.content_modified_notice, f.embedded_light
             FROM files f JOIN documents d ON d.id = f.document_id
             WHERE f.rel_path = 'synced.pdf'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(passage_cnt, 1);
    assert_eq!(notice, 0);
    assert_eq!(embedded_light, 1);
}
