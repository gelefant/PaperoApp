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

    Ok(())
}
