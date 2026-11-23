use papero_core::db::repos::docs::{create_heuristic_document, insert_file_record, NewFileRecord};
use papero_core::db::repos::roots::{insert_source_root, NewSourceRoot};
use papero_core::db::Database;
use papero_core::metadata::{search_documents_metadata, MetadataSearchFilter};
use std::time::Instant;
use tempfile::tempdir;

#[test]
fn metadata_search_5000_docs_benchmark() {
    let dir = tempdir().unwrap();
    let db = Database::open(&dir.path().join("papero.db"), 2).unwrap();
    let root_id = {
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: "bench-root".into(),
                label: "BenchLib".into(),
                root_path: dir.path().to_string_lossy().into_owned(),
                is_network_or_cloud: false,
            },
        )
        .unwrap()
        .id
    };

    {
        let w = db.writer();
        let tx = w.unchecked_transaction().unwrap();
        for i in 0..5000 {
            let title = if i == 2499 {
                "Optimal Approximations on Poincaré Manifolds"
            } else {
                "Spectral Theory and Differential Equations on Manifolds"
            };
            let doc_id = create_heuristic_document(&tx, &format!("{title} #{i}")).unwrap();
            let year = 1960 + (i % 65) as i64;
            let author_norm = if i == 2499 { "paul erdos" } else { "jean dieudonne" };
            let msc = if i % 10 == 0 { "49F20" } else { "35J15" };
            tx.execute(
                "UPDATE documents SET authors_norm_summary = ?1, year = ?2, msc_primary = ?3 WHERE id = ?4",
                rusqlite::params![author_norm, year, msc, doc_id],
            )
            .unwrap();
            insert_file_record(
                &tx,
                &NewFileRecord {
                    document_id: doc_id,
                    root_id: &root_id,
                    rel_path: &format!("book_{i:04}.pdf"),
                    format: "pdf",
                    size_bytes: 1024,
                    mtime_ns: i as i64,
                    blake3_hash: &format!("{i:064x}"),
                    index_state: "ready",
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }

    let r = db.reader();
    let filter = MetadataSearchFilter {
        text: Some("Poincare".into()),
        author: Some("Erdős".into()),
        year_from: Some(1960),
        year_to: Some(2030),
        msc_prefix: None,
        tag: None,
        root_id: Some(root_id),
        format: Some("pdf".into()),
        sort_by: Some("title".into()),
        ..Default::default()
    };

    let t0 = Instant::now();
    let hits = search_documents_metadata(&r, &filter).unwrap();
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("metadata_search_5000_docs: hits={} elapsed_ms={:.2}", hits.len(), elapsed_ms);
    assert_eq!(hits.len(), 1);
    let budget_ms = if cfg!(debug_assertions) { 1500.0 } else { 100.0 };
    assert!(
        elapsed_ms < budget_ms,
        "unexpectedly slow search: {elapsed_ms:.2} ms (budget {budget_ms:.0} ms)"
    );
}
