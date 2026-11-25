use crate::error::Result;
use rusqlite::Connection;

const SCHEMA_V1: &str = include_str!("schema.sql");

/// Applies required SQLite performance & safety pragmas on any opened connection.
pub fn apply_pragmas(conn: &Connection, read_only: bool) -> Result<()> {
    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;
        PRAGMA temp_store = MEMORY;
        PRAGMA busy_timeout = 5000;
        PRAGMA mmap_size = 134217728;
        ",
    )?;
    if read_only {
        conn.execute_batch("PRAGMA cache_size = -4096;")?;
    } else {
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA cache_size = -8192;
            PRAGMA wal_autocheckpoint = 2000;
            ",
        )?;
    }
    Ok(())
}

/// Runs versioned schema migrations on the writer connection.
pub fn migrate(conn: &Connection) -> Result<()> {
    apply_pragmas(conn, false)?;

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        );
        ",
    )?;

    let current_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if current_version < 1 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(SCHEMA_V1)?;
        let now = chrono::Utc::now().timestamp();
        tx.execute(
            "INSERT INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![1i64, now],
        )?;
        tx.commit()?;
    }

    if current_version < 2 {
        let mut st = conn.prepare("PRAGMA table_info(documents)")?;
        let has_verified_col = st
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(std::result::Result::ok)
            .any(|name| name == "metadata_verified");
        drop(st);
        let tx = conn.unchecked_transaction()?;
        if !has_verified_col {
            tx.execute_batch(
                "ALTER TABLE documents ADD COLUMN metadata_verified INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        let now = chrono::Utc::now().timestamp();
        tx.execute(
            "INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![2i64, now],
        )?;
        tx.commit()?;
    }

    if current_version < 3 {
        let tx = conn.unchecked_transaction()?;
        // Purge stale negative/legacy query cache entries so upgraded structured queries run fresh
        tx.execute_batch(
            "DELETE FROM metadata_http_cache
             WHERE status_code >= 400
                OR lookup_key LIKE 'q:%'
                OR lookup_key LIKE 'auyr:%';",
        )?;
        let now = chrono::Utc::now().timestamp();
        tx.execute(
            "INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![3i64, now],
        )?;
        tx.commit()?;
    }

    if current_version < 4 {
        let tx = conn.unchecked_transaction()?;
        // Purge stale negative/legacy query cache entries so Wave 2 ligature/OT1/structured queries run fresh
        tx.execute_batch(
            "DELETE FROM metadata_http_cache
             WHERE status_code >= 400
                OR lookup_key LIKE 'q:%'
                OR lookup_key LIKE 'qp:%'
                OR lookup_key LIKE 'qcore:%'
                OR lookup_key LIKE 'ts:%'
                OR lookup_key LIKE 'match:%'
                OR lookup_key LIKE 'auyr:%';",
        )?;
        let now = chrono::Utc::now().timestamp();
        tx.execute(
            "INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![4i64, now],
        )?;
        tx.commit()?;
    }

    if current_version < 5 {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_docs_updated_at ON documents(updated_at DESC, id DESC);
             CREATE INDEX IF NOT EXISTS idx_passages_doc_page ON passages(document_id, page_ord, chunk_index);
             CREATE INDEX IF NOT EXISTS idx_metadata_candidates_doc ON metadata_candidates(document_id);
             CREATE INDEX IF NOT EXISTS idx_index_jobs_stage_status ON index_jobs(stage, status, priority, id);
             UPDATE index_jobs SET status = 'queued', last_error = NULL WHERE stage = 'metadata' AND status = 'failed';",
        )?;
        let now = chrono::Utc::now().timestamp();
        tx.execute(
            "INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![5i64, now],
        )?;
        tx.commit()?;
    }

    if current_version < 6 {
        rebuild_minhashes_and_recompute_duplicates_v6(conn)?;
    }

    if current_version < 7 {
        migrate_v7_annotations_reading_collections(conn)?;
    }

    Ok(())
}

fn migrate_v7_annotations_reading_collections(conn: &Connection) -> Result<()> {
    let existing_cols: std::collections::HashSet<String> = {
        let mut st = conn.prepare("PRAGMA table_info(documents)")?;
        let iter = st.query_map([], |r| r.get::<_, String>(1))?;
        iter.filter_map(std::result::Result::ok).collect()
    };

    let tx = conn.unchecked_transaction()?;
    if !existing_cols.contains("reading_status") {
        tx.execute_batch(
            "ALTER TABLE documents ADD COLUMN reading_status TEXT NOT NULL DEFAULT 'unread';",
        )?;
    }
    if !existing_cols.contains("is_starred") {
        tx.execute_batch(
            "ALTER TABLE documents ADD COLUMN is_starred INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    if !existing_cols.contains("published_notice_json") {
        tx.execute_batch("ALTER TABLE documents ADD COLUMN published_notice_json TEXT;")?;
    }

    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS file_annotations (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            blake3_hash     TEXT NOT NULL,
            page_ord        INTEGER NOT NULL,
            kind            TEXT NOT NULL DEFAULT 'highlight',
            color           TEXT NOT NULL DEFAULT 'yellow',
            rects_json      TEXT NOT NULL DEFAULT '[]',
            quoted_text     TEXT NOT NULL DEFAULT '',
            note_markdown   TEXT NOT NULL DEFAULT '',
            search_norm     TEXT NOT NULL DEFAULT '',
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_file_annotations_hash_page ON file_annotations(blake3_hash, page_ord, id);
        CREATE INDEX IF NOT EXISTS idx_file_annotations_hash_kind ON file_annotations(blake3_hash, kind);

        CREATE TABLE IF NOT EXISTS file_reading_state (
            blake3_hash     TEXT PRIMARY KEY,
            last_page_ord   INTEGER NOT NULL DEFAULT 1,
            last_opened_at  INTEGER,
            zoom_mode       TEXT NOT NULL DEFAULT 'fit-width',
            custom_zoom     REAL NOT NULL DEFAULT 1.0,
            rotation_deg    INTEGER NOT NULL DEFAULT 0,
            updated_at      INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_file_reading_state_opened ON file_reading_state(last_opened_at DESC);

        CREATE TABLE IF NOT EXISTS collections (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL,
            color_hex   TEXT,
            sort_order  INTEGER NOT NULL DEFAULT 0,
            created_at  INTEGER NOT NULL,
            updated_at  INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS collection_documents (
            collection_id   INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
            document_id     INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            position        INTEGER NOT NULL DEFAULT 0,
            added_at        INTEGER NOT NULL,
            PRIMARY KEY (collection_id, document_id)
        );
        CREATE INDEX IF NOT EXISTS idx_collection_docs_doc ON collection_documents(document_id);

        CREATE TABLE IF NOT EXISTS saved_searches (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            name        TEXT NOT NULL,
            search_mode TEXT NOT NULL DEFAULT 'metadata',
            query_text  TEXT NOT NULL DEFAULT '',
            filter_json TEXT NOT NULL DEFAULT '{}',
            sort_order  INTEGER NOT NULL DEFAULT 0,
            created_at  INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS document_citations (
            citing_doc_id   INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            cited_doc_id    INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
            source          TEXT NOT NULL,
            confidence      REAL NOT NULL DEFAULT 1.0,
            PRIMARY KEY (citing_doc_id, cited_doc_id)
        );
        CREATE INDEX IF NOT EXISTS idx_doc_citations_cited ON document_citations(cited_doc_id);
        CREATE INDEX IF NOT EXISTS idx_docs_reading_status ON documents(reading_status);
        CREATE INDEX IF NOT EXISTS idx_docs_starred ON documents(is_starred) WHERE is_starred != 0;",
    )?;

    let now = chrono::Utc::now().timestamp();
    tx.execute(
        "INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (?1, ?2)",
        rusqlite::params![7i64, now],
    )?;
    tx.commit()?;
    Ok(())
}

fn rebuild_minhashes_and_recompute_duplicates_v6(conn: &Connection) -> Result<()> {
    use crate::dedup::detector::{refresh_user_decided_pair_hashes, run_duplicate_detection_conn};
    use crate::dedup::{compute_document_minhash_from_pages, pack_minhash};

    let mut updates: Vec<(i64, Option<Vec<u8>>)> = Vec::new();
    {
        let mut stmt = conn.prepare(
            "WITH primary_files AS (
                 SELECT document_id, MIN(id) AS file_id
                 FROM files
                 GROUP BY document_id
             )
             SELECT pf.document_id, COALESCE(f.page_count, 0), p.page_ord, p.text_content
             FROM primary_files pf
             JOIN files f ON f.id = pf.file_id
             LEFT JOIN passages p
               ON p.file_id = pf.file_id
              AND p.is_alt_formula = 0
              AND (
                    COALESCE(f.page_count, 0) < 45
                    OR (
                        p.page_ord >= MIN(35, MAX(14, (f.page_count * 18) / 100))
                        AND p.page_ord <= MIN((f.page_count * 90) / 100, MIN(35, MAX(14, (f.page_count * 18) / 100)) + 90)
                    )
                  )
             ORDER BY pf.document_id ASC, p.page_ord ASC, p.chunk_index ASC",
        )?;

        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?.max(0) as u32,
                r.get::<_, Option<i64>>(2)?.map(|v| v.max(0) as u32),
                r.get::<_, Option<String>>(3)?,
            ))
        })?;

        let mut cur_doc: Option<(i64, u32)> = None;
        let mut cur_pages: Vec<(u32, String)> = Vec::new();

        let flush_doc = |doc_id: i64,
                         page_count: u32,
                         pages: &[(u32, String)],
                         out: &mut Vec<(i64, Option<Vec<u8>>)>| {
            let blob = compute_document_minhash_from_pages(
                pages.iter().map(|(ord, txt)| (*ord, txt.as_str())),
                page_count,
            )
            .map(|sig| pack_minhash(&sig));
            out.push((doc_id, blob));
        };

        for row in rows {
            let (doc_id, page_count, ord_opt, txt_opt) = row?;
            if let Some((prev_id, prev_pc)) = cur_doc {
                if prev_id != doc_id {
                    flush_doc(prev_id, prev_pc, &cur_pages, &mut updates);
                    cur_pages.clear();
                    cur_doc = Some((doc_id, page_count));
                }
            } else {
                cur_doc = Some((doc_id, page_count));
            }

            if let (Some(ord), Some(txt)) = (ord_opt, txt_opt) {
                if page_count >= 45 {
                    let start = ((page_count * 18) / 100).clamp(14, 35);
                    let end = ((page_count * 90) / 100).min(start + 90).max(start + 1);
                    if ord < start || ord > end {
                        continue;
                    }
                } else if page_count >= 18 {
                    let start = (page_count / 5).max(4);
                    let end = page_count.saturating_sub(2).max(start);
                    if ord < start || ord > end {
                        continue;
                    }
                }
                if let Some((last_ord, last_txt)) = cur_pages.last_mut() {
                    if *last_ord == ord {
                        last_txt.push('\n');
                        last_txt.push_str(&txt);
                        continue;
                    }
                }
                cur_pages.push((ord, txt));
            }
        }

        if let Some((last_id, last_pc)) = cur_doc {
            flush_doc(last_id, last_pc, &cur_pages, &mut updates);
        }
    }

    {
        let tx = conn.unchecked_transaction()?;
        {
            let mut upd_stmt =
                tx.prepare("UPDATE documents SET minhash_sig = ?1 WHERE id = ?2")?;
            for (doc_id, blob) in &updates {
                upd_stmt.execute(rusqlite::params![blob, doc_id])?;
            }
        }
        // Purge unresolved false-positive pending groups & stale cached pair results while preserving
        // all explicit user resolutions ('distinct', 'merged', 'versions', 'editions', 'info_merged').
        tx.execute_batch(
            "DELETE FROM duplicate_groups WHERE status = 'pending' AND group_level >= 2;
             DELETE FROM duplicate_evaluated_pairs WHERE decision IN ('pending', 'clean');",
        )?;
        let now = chrono::Utc::now().timestamp();
        tx.execute(
            "INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![6i64, now],
        )?;
        tx.commit()?;
    }

    refresh_user_decided_pair_hashes(conn)?;
    if updates.len() >= 2 {
        let _ = run_duplicate_detection_conn(conn)?;
    }

    Ok(())
}
