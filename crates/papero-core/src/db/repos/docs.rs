use crate::error::Result;
use crate::extract::types::{unpack_bboxes, NormalizedRect, OutlineItem};
use crate::fs::roots::resolve_rel_path;
use crate::index::bm25::Bm25Hit;
use crate::text_norm::normalize_math_text;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct KnownFileRecord {
    pub file_id: i64,
    pub document_id: i64,
    pub root_id: String,
    pub rel_path: String,
    pub format: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    pub blake3_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassageInsert {
    pub document_id: i64,
    pub file_id: i64,
    pub page_ord: u32,
    pub printed_label: Option<String>,
    pub chunk_index: u32,
    pub is_alt_formula: bool,
    pub token_count: usize,
    pub text_content: String,
    pub bboxes_blob: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PassageView {
    pub id: u64,
    pub document_id: i64,
    pub file_id: i64,
    pub page_ord: u32,
    pub printed_label: Option<String>,
    pub text_content: String,
    pub bboxes: Vec<NormalizedRect>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePageInsert {
    pub file_id: i64,
    pub page_ord: u32,
    pub printed_label: Option<String>,
    pub width_pts: f32,
    pub height_pts: f32,
    pub text_source: String,
}

pub fn load_known_files_for_roots(
    conn: &Connection,
    root_ids: &[String],
) -> Result<Vec<KnownFileRecord>> {
    if root_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = root_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, document_id, root_id, rel_path, format, size_bytes, mtime_ns, blake3_hash
         FROM files WHERE root_id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let params_vec: Vec<&dyn rusqlite::ToSql> =
        root_ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let rows = stmt.query_map(params_vec.as_slice(), |r| {
        Ok(KnownFileRecord {
            file_id: r.get(0)?,
            document_id: r.get(1)?,
            root_id: r.get(2)?,
            rel_path: r.get(3)?,
            format: r.get(4)?,
            size_bytes: r.get::<_, i64>(5)? as u64,
            mtime_ns: r.get(6)?,
            blake3_hash: r.get(7)?,
        })
    })?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn load_all_known_hashes(conn: &Connection) -> Result<Vec<KnownFileRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, document_id, root_id, rel_path, format, size_bytes, mtime_ns, blake3_hash
         FROM files",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(KnownFileRecord {
            file_id: r.get(0)?,
            document_id: r.get(1)?,
            root_id: r.get(2)?,
            rel_path: r.get(3)?,
            format: r.get(4)?,
            size_bytes: r.get::<_, i64>(5)? as u64,
            mtime_ns: r.get(6)?,
            blake3_hash: r.get(7)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn create_heuristic_document(conn: &Connection, title: &str) -> Result<i64> {
    let now = chrono::Utc::now().timestamp();
    let title_norm = normalize_math_text(title);
    conn.execute(
        "INSERT INTO documents (
            work_type, title, title_norm, authors_norm_summary,
            metadata_source, created_at, updated_at
         ) VALUES ('article', ?1, ?2, '', 'heuristic', ?3, ?3)",
        params![title, title_norm, now],
    )?;
    Ok(conn.last_insert_rowid())
}

#[derive(Debug, Clone)]
pub struct NewFileRecord<'a> {
    pub document_id: i64,
    pub root_id: &'a str,
    pub rel_path: &'a str,
    pub format: &'a str,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    pub blake3_hash: &'a str,
    pub index_state: &'a str,
}

pub fn insert_file_record(conn: &Connection, rec: &NewFileRecord<'_>) -> Result<i64> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO files (
            document_id, root_id, rel_path, format, size_bytes, mtime_ns,
            blake3_hash, is_available, index_state, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?9)",
        params![
            rec.document_id,
            rec.root_id,
            rec.rel_path,
            rec.format,
            rec.size_bytes as i64,
            rec.mtime_ns,
            rec.blake3_hash,
            rec.index_state,
            now
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_file_location(
    conn: &Connection,
    file_id: i64,
    new_root_id: &str,
    new_rel_path: &str,
    size_bytes: u64,
    mtime_ns: i64,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE files SET root_id = ?1, rel_path = ?2, size_bytes = ?3, mtime_ns = ?4,
                is_available = 1, updated_at = ?5
         WHERE id = ?6",
        params![
            new_root_id,
            new_rel_path,
            size_bytes as i64,
            mtime_ns,
            now,
            file_id
        ],
    )?;
    Ok(())
}

pub fn enqueue_index_job(conn: &Connection, file_id: i64, stage: &str, priority: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO index_jobs (file_id, stage, priority, status, attempts)
         VALUES (?1, ?2, ?3, 'queued', 0)
         ON CONFLICT(file_id, stage) DO UPDATE SET
            status = 'queued',
            last_error = NULL",
        params![file_id, stage, priority],
    )?;
    Ok(())
}

pub fn get_file_passage_ids(conn: &Connection, file_id: i64) -> Result<Vec<u64>> {
    let mut stmt = conn.prepare("SELECT id FROM passages WHERE file_id = ?1")?;
    let rows = stmt.query_map(params![file_id], |r| Ok(r.get::<_, i64>(0)? as u64))?;
    let mut ids = Vec::new();
    for r in rows {
        ids.push(r?);
    }
    Ok(ids)
}

pub fn get_document_passage_ids(conn: &Connection, document_id: i64) -> Result<Vec<u64>> {
    let mut stmt = conn.prepare("SELECT id FROM passages WHERE document_id = ?1")?;
    let rows = stmt.query_map(params![document_id], |r| Ok(r.get::<_, i64>(0)? as u64))?;
    let mut ids = Vec::new();
    for r in rows {
        ids.push(r?);
    }
    Ok(ids)
}

pub fn replace_file_pages_and_passages(
    conn: &Connection,
    file_id: i64,
    page_count: u32,
    has_text_layer: bool,
    outline_json: Option<&str>,
    pages: &[FilePageInsert],
    passages: &[PassageInsert],
) -> Result<Vec<u64>> {
    replace_file_content_inner(
        conn,
        file_id,
        page_count,
        has_text_layer,
        outline_json,
        false,
        None,
        pages,
        passages,
    )
}

/// Replaces `file_pages` and non-alt passages (`is_alt_formula = 0`) for `file_id`
/// with OCR-derived pages and passages, preserving any user-imported LaTeX/Nougat passages.
pub fn replace_file_ocr_pages_and_passages(
    conn: &Connection,
    file_id: i64,
    page_count: u32,
    pages: &[FilePageInsert],
    passages: &[PassageInsert],
) -> Result<Vec<u64>> {
    replace_file_content_inner(
        conn,
        file_id,
        page_count,
        true,
        None,
        true,
        Some("done"),
        pages,
        passages,
    )
}

#[allow(clippy::too_many_arguments)]
fn replace_file_content_inner(
    conn: &Connection,
    file_id: i64,
    page_count: u32,
    has_text_layer: bool,
    outline_json: Option<&str>,
    preserve_alt_formulas: bool,
    ocr_status: Option<&str>,
    pages: &[FilePageInsert],
    passages: &[PassageInsert],
) -> Result<Vec<u64>> {
    let now = chrono::Utc::now().timestamp();
    let tx = conn.unchecked_transaction()?;

    tx.execute("DELETE FROM file_pages WHERE file_id = ?1", params![file_id])?;
    if preserve_alt_formulas {
        tx.execute(
            "DELETE FROM passages WHERE file_id = ?1 AND is_alt_formula = 0",
            params![file_id],
        )?;
    } else {
        tx.execute("DELETE FROM passages WHERE file_id = ?1", params![file_id])?;
    }

    {
        let mut p_stmt = tx.prepare_cached(
            "INSERT INTO file_pages (file_id, page_ord, printed_label, width_pts, height_pts, text_source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for pg in pages {
            p_stmt.execute(params![
                pg.file_id,
                pg.page_ord as i64,
                pg.printed_label,
                pg.width_pts as f64,
                pg.height_pts as f64,
                pg.text_source,
            ])?;
        }
    }

    let mut inserted_ids = Vec::with_capacity(passages.len());
    {
        let mut c_stmt = tx.prepare_cached(
            "INSERT INTO passages (
                document_id, file_id, page_ord, printed_label, chunk_index,
                is_alt_formula, token_count, text_content, bboxes_blob
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for ch in passages {
            c_stmt.execute(params![
                ch.document_id,
                ch.file_id,
                ch.page_ord as i64,
                ch.printed_label,
                ch.chunk_index as i64,
                ch.is_alt_formula as i64,
                ch.token_count as i64,
                ch.text_content,
                ch.bboxes_blob,
            ])?;
            inserted_ids.push(tx.last_insert_rowid() as u64);
        }
    }

    tx.execute(
        "UPDATE files SET
            page_count = ?1,
            has_text_layer = ?2,
            outline_json = COALESCE(?3, outline_json),
            ocr_status = COALESCE(?4, ocr_status),
            embedded_light = 0,
            embedded_quality = 0,
            index_state = 'bm25_done',
            error_message = NULL,
            updated_at = ?5
         WHERE id = ?6",
        params![
            page_count as i64,
            has_text_layer as i64,
            outline_json,
            ocr_status,
            now,
            file_id
        ],
    )?;

    tx.commit()?;
    Ok(inserted_ids)
}

pub fn get_file_non_alt_passage_ids(conn: &Connection, file_id: i64) -> Result<Vec<u64>> {
    let mut stmt =
        conn.prepare("SELECT id FROM passages WHERE file_id = ?1 AND is_alt_formula = 0")?;
    let rows = stmt.query_map(params![file_id], |r| Ok(r.get::<_, i64>(0)? as u64))?;
    let mut ids = Vec::new();
    for r in rows {
        ids.push(r?);
    }
    Ok(ids)
}

pub fn set_file_ocr_status(
    conn: &Connection,
    file_id: i64,
    ocr_status: &str,
    err: Option<&str>,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE files SET ocr_status = ?1, error_message = COALESCE(?2, error_message), updated_at = ?3 WHERE id = ?4",
        params![ocr_status, err, now, file_id],
    )?;
    Ok(())
}

pub fn mark_file_error(
    conn: &Connection,
    file_id: i64,
    index_state: &str,
    message: &str,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE files SET index_state = ?1, error_message = ?2, updated_at = ?3 WHERE id = ?4",
        params![index_state, message, now, file_id],
    )?;
    Ok(())
}

pub fn count_files_for_document(conn: &Connection, document_id: i64) -> Result<i64> {
    let cnt: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE document_id = ?1",
        params![document_id],
        |r| r.get(0),
    )?;
    Ok(cnt)
}

pub fn get_passage_by_id(conn: &Connection, passage_id: u64) -> Result<Option<PassageView>> {
    conn.query_row(
        "SELECT id, document_id, file_id, page_ord, printed_label, text_content, bboxes_blob
         FROM passages WHERE id = ?1",
        params![passage_id as i64],
        |r| {
            let blob: Vec<u8> = r.get(6)?;
            Ok(PassageView {
                id: r.get::<_, i64>(0)? as u64,
                document_id: r.get(1)?,
                file_id: r.get(2)?,
                page_ord: r.get::<_, i64>(3)? as u32,
                printed_label: r.get(4)?,
                text_content: r.get(5)?,
                bboxes: unpack_bboxes(&blob),
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn record_level1_identical_copy(
    conn: &Connection,
    document_id: i64,
    blake3_hash: &str,
    file_id: i64,
) -> Result<bool> {
    let reason = format!("Identical BLAKE3 {blake3_hash}");
    let existing_group: Option<i64> = conn
        .query_row(
            "SELECT id FROM duplicate_groups WHERE group_level = 1 AND reason_detail = ?1",
            params![reason],
            |r| r.get(0),
        )
        .optional()?;

    let now = chrono::Utc::now().timestamp();
    let (group_id, is_new) = if let Some(gid) = existing_group {
        (gid, false)
    } else {
        conn.execute(
            "INSERT INTO duplicate_groups (group_level, max_similarity, reason_detail, status, created_at, resolved_at)
             VALUES (1, 1.0, ?1, 'info_merged', ?2, ?2)",
            params![reason, now],
        )?;
        let gid = conn.last_insert_rowid();
        // Add all existing files for this document as members
        let mut stmt = conn.prepare("SELECT id FROM files WHERE document_id = ?1")?;
        let existing_fids: Vec<i64> = stmt
            .query_map(params![document_id], |r| r.get(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for fid in existing_fids {
            conn.execute(
                "INSERT OR IGNORE INTO duplicate_group_members (group_id, document_id, file_id)
                 VALUES (?1, ?2, ?3)",
                params![gid, document_id, fid],
            )?;
        }
        (gid, true)
    };

    conn.execute(
        "INSERT OR IGNORE INTO duplicate_group_members (group_id, document_id, file_id)
         VALUES (?1, ?2, ?3)",
        params![group_id, document_id, file_id],
    )?;

    Ok(is_new)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentListItem {
    pub document_id: i64,
    pub file_id: i64,
    pub root_id: String,
    pub root_label: String,
    #[serde(default)]
    pub work_type: String,
    pub title: String,
    pub authors_summary: String,
    pub year: Option<i64>,
    pub format: String,
    pub rel_path: String,
    pub page_count: Option<u32>,
    pub is_available: bool,
    pub index_state: String,
    pub error_message: Option<String>,
    pub msc_primary: Option<String>,
    pub msc_primary_status: String,
    pub has_text_layer: bool,
    pub ocr_status: String,
    #[serde(default)]
    pub metadata_source: Option<String>,
    #[serde(default)]
    pub metadata_verified: bool,
}

#[derive(Debug, Clone)]
pub struct FileLocation {
    pub file_id: i64,
    pub document_id: i64,
    pub root_id: String,
    pub root_path: String,
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub format: String,
    pub size_bytes: u64,
    pub blake3_hash: String,
    pub is_available: bool,
    pub page_count: Option<u32>,
    pub outline_json: Option<String>,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilePageInfo {
    pub page_ord: u32,
    pub printed_label: Option<String>,
    pub width_pts: f32,
    pub height_pts: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileViewerInfo {
    pub file_id: i64,
    pub document_id: i64,
    pub title: String,
    pub format: String,
    pub byte_size: u64,
    pub page_count: u32,
    pub is_available: bool,
    pub abs_path: String,
    pub pages: Vec<FilePageInfo>,
    pub outline: Vec<OutlineItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentSearchPassageHit {
    pub passage_id: u64,
    pub file_id: i64,
    pub page_ord: u32,
    pub printed_label: Option<String>,
    pub score: f32,
    pub text_content: String,
    pub bboxes: Vec<NormalizedRect>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentSearchDocResult {
    pub document_id: i64,
    pub file_id: i64,
    #[serde(default)]
    pub work_type: String,
    pub title: String,
    pub format: String,
    pub rel_path: String,
    pub is_available: bool,
    pub max_score: f32,
    pub passages: Vec<ContentSearchPassageHit>,
}

pub fn list_documents_for_ui(conn: &Connection) -> Result<Vec<DocumentListItem>> {
    let mut stmt = conn.prepare(
        "SELECT d.id, f.id, f.root_id, sr.label, d.work_type, d.title,
                COALESCE(
                    (SELECT GROUP_CONCAT(
                        CASE WHEN TRIM(given_name) = '' THEN family_name
                             WHEN TRIM(family_name) = '' THEN given_name
                             ELSE family_name || ', ' || given_name END,
                        '; '
                     )
                     FROM (
                        SELECT family_name, given_name
                        FROM document_authors
                        WHERE document_id = d.id
                        ORDER BY CASE WHEN role = 'author' THEN 0 ELSE 1 END, position ASC
                     )),
                    NULLIF(d.authors_norm_summary, ''),
                    ''
                ) AS authors_display,
                d.year, f.format, f.rel_path, f.page_count, f.is_available,
                f.index_state, f.error_message, d.msc_primary, d.msc_primary_status,
                f.has_text_layer, f.ocr_status, d.metadata_source, d.metadata_verified
         FROM files f
         JOIN documents d ON d.id = f.document_id
         JOIN source_roots sr ON sr.id = f.root_id
         GROUP BY d.id
         ORDER BY d.updated_at DESC, d.id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(DocumentListItem {
            document_id: r.get(0)?,
            file_id: r.get(1)?,
            root_id: r.get(2)?,
            root_label: r.get(3)?,
            work_type: r.get(4)?,
            title: r.get(5)?,
            authors_summary: r.get(6)?,
            year: r.get(7)?,
            format: r.get(8)?,
            rel_path: r.get(9)?,
            page_count: r.get::<_, Option<i64>>(10)?.map(|v| v as u32),
            is_available: r.get::<_, i64>(11)? != 0,
            index_state: r.get(12)?,
            error_message: r.get(13)?,
            msc_primary: r.get(14)?,
            msc_primary_status: r.get(15)?,
            has_text_layer: r.get::<_, i64>(16)? != 0,
            ocr_status: r.get(17)?,
            metadata_source: r.get(18)?,
            metadata_verified: r.get::<_, i64>(19)? != 0,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn get_file_location(conn: &Connection, file_id: i64) -> Result<Option<FileLocation>> {
    let row = conn
        .query_row(
            "SELECT f.id, f.document_id, f.root_id, sr.root_path, f.rel_path, f.format,
                    f.size_bytes, f.blake3_hash, f.is_available, f.page_count, f.outline_json, d.title
             FROM files f
             JOIN source_roots sr ON sr.id = f.root_id
             JOIN documents d ON d.id = f.document_id
             WHERE f.id = ?1",
            params![file_id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)? as u64,
                    r.get::<_, String>(7)?,
                    r.get::<_, i64>(8)? != 0,
                    r.get::<_, Option<i64>>(9)?.map(|v| v as u32),
                    r.get::<_, Option<String>>(10)?,
                    r.get::<_, String>(11)?,
                ))
            },
        )
        .optional()?;

    let Some((
        fid,
        doc_id,
        root_id,
        root_path,
        rel_path,
        format,
        size_bytes,
        blake3_hash,
        is_available,
        page_count,
        outline_json,
        title,
    )) = row
    else {
        return Ok(None);
    };

    let abs_path = resolve_rel_path(std::path::Path::new(&root_path), &rel_path)?;
    Ok(Some(FileLocation {
        file_id: fid,
        document_id: doc_id,
        root_id,
        root_path,
        rel_path,
        abs_path,
        format,
        size_bytes,
        blake3_hash,
        is_available,
        page_count,
        outline_json,
        title,
    }))
}

pub fn get_file_viewer_info(conn: &Connection, file_id: i64) -> Result<Option<FileViewerInfo>> {
    let Some(loc) = get_file_location(conn, file_id)? else {
        return Ok(None);
    };

    let mut stmt = conn.prepare(
        "SELECT page_ord, printed_label, width_pts, height_pts
         FROM file_pages WHERE file_id = ?1 ORDER BY page_ord ASC",
    )?;
    let pages: Vec<FilePageInfo> = stmt
        .query_map(params![file_id], |r| {
            Ok(FilePageInfo {
                page_ord: r.get::<_, i64>(0)? as u32,
                printed_label: r.get(1)?,
                width_pts: r.get::<_, f64>(2)? as f32,
                height_pts: r.get::<_, f64>(3)? as f32,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let outline: Vec<OutlineItem> = loc
        .outline_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let page_count = loc.page_count.unwrap_or(pages.len() as u32);
    let byte_size = if loc.size_bytes > 0 {
        loc.size_bytes
    } else {
        std::fs::metadata(&loc.abs_path).map(|m| m.len()).unwrap_or(0)
    };

    Ok(Some(FileViewerInfo {
        file_id: loc.file_id,
        document_id: loc.document_id,
        title: loc.title,
        format: loc.format,
        byte_size,
        page_count,
        is_available: loc.is_available,
        abs_path: loc.abs_path.to_string_lossy().into_owned(),
        pages,
        outline,
    }))
}

/// Hydrates BM25 hits from SQLite and groups them by `document_id` (score = max of passages).
pub fn hydrate_bm25_results(
    conn: &Connection,
    hits: &[Bm25Hit],
) -> Result<Vec<ContentSearchDocResult>> {
    hydrate_bm25_results_filtered(conn, hits, None)
}

/// Hydrates BM25 hits with an optional `work_category` filter (`"paper"` | `"book"` | `"all"`).
pub fn hydrate_bm25_results_filtered(
    conn: &Connection,
    hits: &[Bm25Hit],
    work_category: Option<&str>,
) -> Result<Vec<ContentSearchDocResult>> {
    if hits.is_empty() {
        return Ok(Vec::new());
    }

    let score_by_pid: HashMap<u64, f32> = hits.iter().map(|h| (h.passage_id, h.score)).collect();
    let placeholders = hits
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(",");
    let cat_clause = match work_category.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("book") => " AND d.work_type = 'book'",
        Some("paper") => " AND d.work_type != 'book'",
        _ => "",
    };
    let sql = format!(
        "SELECT p.id, p.document_id, p.file_id, p.page_ord, p.printed_label,
                p.text_content, p.bboxes_blob, d.work_type, d.title, f.format, f.rel_path, f.is_available
         FROM passages p
         JOIN documents d ON d.id = p.document_id
         JOIN files f ON f.id = p.file_id
         WHERE p.id IN ({placeholders}){cat_clause}"
    );

    let mut stmt = conn.prepare(&sql)?;
    let pids: Vec<i64> = hits.iter().map(|h| h.passage_id as i64).collect();
    let param_refs: Vec<&dyn rusqlite::ToSql> =
        pids.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

    let mut grouped: HashMap<i64, ContentSearchDocResult> = HashMap::new();
    let rows = stmt.query_map(param_refs.as_slice(), |r| {
        let pid = r.get::<_, i64>(0)? as u64;
        let doc_id = r.get::<_, i64>(1)?;
        let file_id = r.get::<_, i64>(2)?;
        let page_ord = r.get::<_, i64>(3)? as u32;
        let printed_label: Option<String> = r.get(4)?;
        let text_content: String = r.get(5)?;
        let blob: Vec<u8> = r.get(6)?;
        let work_type: String = r.get(7)?;
        let title: String = r.get(8)?;
        let format: String = r.get(9)?;
        let rel_path: String = r.get(10)?;
        let is_available: bool = r.get::<_, i64>(11)? != 0;
        Ok((
            pid,
            doc_id,
            file_id,
            page_ord,
            printed_label,
            text_content,
            blob,
            work_type,
            title,
            format,
            rel_path,
            is_available,
        ))
    })?;

    for row in rows {
        let (
            pid,
            doc_id,
            file_id,
            page_ord,
            printed_label,
            text_content,
            blob,
            work_type,
            title,
            format,
            rel_path,
            is_available,
        ) = row?;
        let score = *score_by_pid.get(&pid).unwrap_or(&0.0);
        let entry = grouped.entry(doc_id).or_insert_with(|| ContentSearchDocResult {
            document_id: doc_id,
            file_id,
            work_type,
            title,
            format,
            rel_path,
            is_available,
            max_score: score,
            passages: Vec::new(),
        });
        if score > entry.max_score {
            entry.max_score = score;
        }
        entry.passages.push(ContentSearchPassageHit {
            passage_id: pid,
            file_id,
            page_ord,
            printed_label,
            score,
            text_content,
            bboxes: unpack_bboxes(&blob),
        });
    }

    let mut results: Vec<ContentSearchDocResult> = grouped.into_values().collect();
    for doc in &mut results {
        doc.passages.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    results.sort_by(|a, b| {
        b.max_score
            .partial_cmp(&a.max_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(results)
}
