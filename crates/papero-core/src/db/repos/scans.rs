use crate::error::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanSummary {
    pub scan_id: i64,
    pub status: String,
    pub cnt_unchanged: usize,
    pub cnt_new: usize,
    pub cnt_identical_copy: usize,
    pub cnt_moved_renamed: usize,
    pub cnt_modified: usize,
    pub cnt_removed: usize,
    pub cnt_errors: usize,
    pub cnt_dup_groups_new: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRunItem {
    pub category: String,
    pub root_id: Option<String>,
    pub rel_path: String,
    pub detail: Option<String>,
}

pub fn create_scan_run(conn: &Connection) -> Result<i64> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO scan_runs (started_at, status) VALUES (?1, 'running')",
        params![now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn finish_scan_run(conn: &Connection, summary: &ScanSummary) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE scan_runs SET
            finished_at = ?1,
            status = ?2,
            cnt_unchanged = ?3,
            cnt_new = ?4,
            cnt_identical_copy = ?5,
            cnt_moved_renamed = ?6,
            cnt_modified = ?7,
            cnt_removed = ?8,
            cnt_errors = ?9,
            cnt_dup_groups_new = ?10
         WHERE id = ?11",
        params![
            now,
            summary.status,
            summary.cnt_unchanged as i64,
            summary.cnt_new as i64,
            summary.cnt_identical_copy as i64,
            summary.cnt_moved_renamed as i64,
            summary.cnt_modified as i64,
            summary.cnt_removed as i64,
            summary.cnt_errors as i64,
            summary.cnt_dup_groups_new as i64,
            summary.scan_id,
        ],
    )?;
    Ok(())
}

pub fn insert_scan_items(
    conn: &Connection,
    scan_id: i64,
    items: &[ScanRunItem],
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let mut stmt = conn.prepare_cached(
        "INSERT INTO scan_run_items (scan_id, category, root_id, rel_path, detail)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for item in items {
        stmt.execute(params![
            scan_id,
            item.category,
            item.root_id,
            item.rel_path,
            item.detail
        ])?;
    }
    Ok(())
}

pub fn get_scan_items(
    conn: &Connection,
    scan_id: i64,
    category: Option<&str>,
) -> Result<Vec<ScanRunItem>> {
    let mut out = Vec::new();
    if let Some(cat) = category {
        let mut stmt = conn.prepare(
            "SELECT category, root_id, rel_path, detail
             FROM scan_run_items WHERE scan_id = ?1 AND category = ?2",
        )?;
        let rows = stmt.query_map(params![scan_id, cat], |r| {
            Ok(ScanRunItem {
                category: r.get(0)?,
                root_id: r.get(1)?,
                rel_path: r.get(2)?,
                detail: r.get(3)?,
            })
        })?;
        for r in rows {
            out.push(r?);
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT category, root_id, rel_path, detail
             FROM scan_run_items WHERE scan_id = ?1",
        )?;
        let rows = stmt.query_map(params![scan_id], |r| {
            Ok(ScanRunItem {
                category: r.get(0)?,
                root_id: r.get(1)?,
                rel_path: r.get(2)?,
                detail: r.get(3)?,
            })
        })?;
        for r in rows {
            out.push(r?);
        }
    }
    Ok(out)
}
