use crate::error::{CoreError, Result};
use crate::extract::types::NormalizedRect;
use crate::text_norm::normalize_math_text;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAnnotation {
    pub id: i64,
    pub blake3_hash: String,
    pub page_ord: u32,
    /// `'highlight' | 'bookmark' | 'note'`
    pub kind: String,
    /// `'yellow' | 'green' | 'blue' | 'pink'`
    pub color: String,
    pub rects: Vec<NormalizedRect>,
    pub quoted_text: String,
    pub note_markdown: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewFileAnnotation {
    pub page_ord: u32,
    pub kind: String,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default)]
    pub rects: Vec<NormalizedRect>,
    #[serde(default)]
    pub quoted_text: String,
    #[serde(default)]
    pub note_markdown: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFileAnnotation {
    pub color: Option<String>,
    pub note_markdown: Option<String>,
    pub quoted_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotationSearchHit {
    pub annotation: FileAnnotation,
    pub document_id: Option<i64>,
    pub file_id: Option<i64>,
    pub doc_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileReadingState {
    pub blake3_hash: String,
    pub last_page_ord: u32,
    pub last_opened_at: Option<i64>,
    /// `'fit-width' | 'fit-page' | 'custom'`
    pub zoom_mode: String,
    pub custom_zoom: f32,
    /// `0 | 90 | 180 | 270`
    pub rotation_deg: i32,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadingStatePatch {
    pub last_page_ord: Option<u32>,
    pub zoom_mode: Option<String>,
    pub custom_zoom: Option<f32>,
    pub rotation_deg: Option<i32>,
    #[serde(default)]
    pub touch_opened: bool,
}

fn default_color() -> String {
    "yellow".to_string()
}

fn normalize_kind(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "bookmark" => "bookmark",
        "note" => "note",
        _ => "highlight",
    }
}

fn normalize_color(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "green" => "green",
        "blue" => "blue",
        "pink" => "pink",
        _ => "yellow",
    }
}

pub fn normalize_reading_status(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "reading" => "reading",
        "read" => "read",
        _ => "unread",
    }
}

fn normalize_zoom_mode(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "fit-page" => "fit-page",
        "custom" => "custom",
        _ => "fit-width",
    }
}

fn normalize_rotation(deg: i32) -> i32 {
    let rem = deg.rem_euclid(360);
    match (rem + 45) / 90 % 4 {
        1 => 90,
        2 => 180,
        3 => 270,
        _ => 0,
    }
}

fn build_search_norm(quoted: &str, note: &str) -> String {
    let combined = if quoted.trim().is_empty() {
        note.trim().to_string()
    } else if note.trim().is_empty() {
        quoted.trim().to_string()
    } else {
        format!("{} {}", quoted.trim(), note.trim())
    };
    normalize_math_text(&combined)
}

fn lookup_file_hash(conn: &Connection, file_id: i64) -> Result<String> {
    conn.query_row(
        "SELECT blake3_hash FROM files WHERE id = ?1",
        params![file_id],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .ok_or_else(|| CoreError::NotFound(format!("file {file_id}")))
}

fn row_to_annotation(r: &rusqlite::Row<'_>) -> rusqlite::Result<FileAnnotation> {
    let rects_raw: String = r.get(5)?;
    let rects: Vec<NormalizedRect> = serde_json::from_str(&rects_raw).unwrap_or_default();
    Ok(FileAnnotation {
        id: r.get(0)?,
        blake3_hash: r.get(1)?,
        page_ord: r.get::<_, i64>(2)?.max(1) as u32,
        kind: r.get(3)?,
        color: r.get(4)?,
        rects,
        quoted_text: r.get(6)?,
        note_markdown: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

pub fn list_annotations_for_hash(
    conn: &Connection,
    blake3_hash: &str,
) -> Result<Vec<FileAnnotation>> {
    let mut stmt = conn.prepare(
        "SELECT id, blake3_hash, page_ord, kind, color, rects_json, quoted_text, note_markdown, created_at, updated_at
         FROM file_annotations
         WHERE blake3_hash = ?1
         ORDER BY page_ord ASC, created_at ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![blake3_hash], row_to_annotation)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn list_annotations_for_file(conn: &Connection, file_id: i64) -> Result<Vec<FileAnnotation>> {
    let hash = lookup_file_hash(conn, file_id)?;
    list_annotations_for_hash(conn, &hash)
}

pub fn create_annotation_for_file(
    conn: &Connection,
    file_id: i64,
    input: &NewFileAnnotation,
) -> Result<FileAnnotation> {
    let hash = lookup_file_hash(conn, file_id)?;
    create_annotation_for_hash(conn, &hash, input)
}

pub fn create_annotation_for_hash(
    conn: &Connection,
    blake3_hash: &str,
    input: &NewFileAnnotation,
) -> Result<FileAnnotation> {
    let now = chrono::Utc::now().timestamp();
    let kind = normalize_kind(&input.kind);
    let color = normalize_color(&input.color);
    let page_ord = input.page_ord.max(1);
    let rects_json = serde_json::to_string(&input.rects)?;
    let quoted = input.quoted_text.trim().to_string();
    let note = input.note_markdown.trim().to_string();
    let search_norm = build_search_norm(&quoted, &note);

    conn.execute(
        "INSERT INTO file_annotations (
            blake3_hash, page_ord, kind, color, rects_json,
            quoted_text, note_markdown, search_norm, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        params![
            blake3_hash,
            page_ord as i64,
            kind,
            color,
            rects_json,
            quoted,
            note,
            search_norm,
            now
        ],
    )?;
    let id = conn.last_insert_rowid();
    get_annotation_by_id(conn, id)?
        .ok_or_else(|| CoreError::NotFound(format!("annotation {id}")))
}

pub fn get_annotation_by_id(conn: &Connection, id: i64) -> Result<Option<FileAnnotation>> {
    conn.query_row(
        "SELECT id, blake3_hash, page_ord, kind, color, rects_json, quoted_text, note_markdown, created_at, updated_at
         FROM file_annotations WHERE id = ?1",
        params![id],
        row_to_annotation,
    )
    .optional()
    .map_err(Into::into)
}

pub fn update_annotation(
    conn: &Connection,
    id: i64,
    patch: &UpdateFileAnnotation,
) -> Result<FileAnnotation> {
    let existing = get_annotation_by_id(conn, id)?
        .ok_or_else(|| CoreError::NotFound(format!("annotation {id}")))?;
    let color = patch
        .color
        .as_deref()
        .map(normalize_color)
        .unwrap_or(existing.color.as_str())
        .to_string();
    let quoted = patch
        .quoted_text
        .as_deref()
        .map(|s| s.trim().to_string())
        .unwrap_or(existing.quoted_text);
    let note = patch
        .note_markdown
        .as_deref()
        .map(|s| s.trim().to_string())
        .unwrap_or(existing.note_markdown);
    let search_norm = build_search_norm(&quoted, &note);
    let now = chrono::Utc::now().timestamp();

    conn.execute(
        "UPDATE file_annotations
         SET color = ?1, quoted_text = ?2, note_markdown = ?3, search_norm = ?4, updated_at = ?5
         WHERE id = ?6",
        params![color, quoted, note, search_norm, now, id],
    )?;
    get_annotation_by_id(conn, id)?
        .ok_or_else(|| CoreError::NotFound(format!("annotation {id}")))
}

pub fn delete_annotation(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM file_annotations WHERE id = ?1", params![id])?;
    Ok(())
}

/// Toggles a page-level bookmark on `(file_id, page_ord)`. Returns `Some(created)` when added,
/// or `None` when an existing bookmark on that page was removed.
pub fn toggle_page_bookmark(
    conn: &Connection,
    file_id: i64,
    page_ord: u32,
    label: Option<&str>,
) -> Result<Option<FileAnnotation>> {
    let hash = lookup_file_hash(conn, file_id)?;
    let page = page_ord.max(1);
    let existing_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM file_annotations
             WHERE blake3_hash = ?1 AND page_ord = ?2 AND kind = 'bookmark'
             ORDER BY id ASC LIMIT 1",
            params![hash, page as i64],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing_id {
        delete_annotation(conn, id)?;
        return Ok(None);
    }
    let created = create_annotation_for_hash(
        conn,
        &hash,
        &NewFileAnnotation {
            page_ord: page,
            kind: "bookmark".to_string(),
            color: "blue".to_string(),
            rects: Vec::new(),
            quoted_text: String::new(),
            note_markdown: label.unwrap_or("").trim().to_string(),
        },
    )?;
    Ok(Some(created))
}

/// Accent/diacritic-tolerant search over highlights, bookmarks, and Markdown/LaTeX notes.
pub fn search_annotations(
    conn: &Connection,
    query: &str,
    file_id: Option<i64>,
) -> Result<Vec<AnnotationSearchHit>> {
    let norm_q = normalize_math_text(query);
    let tokens: Vec<&str> = norm_q
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Ok(Vec::new());
    }

    let hash_filter = match file_id {
        Some(fid) => Some(lookup_file_hash(conn, fid)?),
        None => None,
    };

    let sql = if hash_filter.is_some() {
        "SELECT a.id, a.blake3_hash, a.page_ord, a.kind, a.color, a.rects_json,
                a.quoted_text, a.note_markdown, a.created_at, a.updated_at, a.search_norm,
                (SELECT f.document_id FROM files f WHERE f.blake3_hash = a.blake3_hash LIMIT 1),
                (SELECT f.id FROM files f WHERE f.blake3_hash = a.blake3_hash LIMIT 1),
                COALESCE((SELECT d.title FROM files f JOIN documents d ON d.id = f.document_id WHERE f.blake3_hash = a.blake3_hash LIMIT 1), '')
         FROM file_annotations a
         WHERE a.blake3_hash = ?1
         ORDER BY a.page_ord ASC, a.updated_at DESC"
    } else {
        "SELECT a.id, a.blake3_hash, a.page_ord, a.kind, a.color, a.rects_json,
                a.quoted_text, a.note_markdown, a.created_at, a.updated_at, a.search_norm,
                (SELECT f.document_id FROM files f WHERE f.blake3_hash = a.blake3_hash LIMIT 1),
                (SELECT f.id FROM files f WHERE f.blake3_hash = a.blake3_hash LIMIT 1),
                COALESCE((SELECT d.title FROM files f JOIN documents d ON d.id = f.document_id WHERE f.blake3_hash = a.blake3_hash LIMIT 1), '')
         FROM file_annotations a
         ORDER BY a.updated_at DESC, a.page_ord ASC"
    };

    let mut stmt = conn.prepare(sql)?;
    let mut rows = if let Some(ref h) = hash_filter {
        stmt.query(params![h])?
    } else {
        stmt.query([])?
    };

    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let search_norm: String = r.get(10)?;
        if !tokens.iter().all(|t| search_norm.contains(t)) {
            continue;
        }
        let ann = row_to_annotation(r)?;
        out.push(AnnotationSearchHit {
            annotation: ann,
            document_id: r.get(11)?,
            file_id: r.get(12)?,
            doc_title: r.get(13)?,
        });
    }
    Ok(out)
}

/// Exports annotations (for a single file or the entire library) as clean Markdown preserving LaTeX.
pub fn export_annotations_markdown(conn: &Connection, file_id: Option<i64>) -> Result<String> {
    if let Some(fid) = file_id {
        let (hash, title): (String, String) = conn
            .query_row(
                "SELECT f.blake3_hash, d.title
                 FROM files f
                 JOIN documents d ON d.id = f.document_id
                 WHERE f.id = ?1",
                params![fid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("file {fid}")))?;
        let anns = list_annotations_for_hash(conn, &hash)?;
        return Ok(format_doc_annotations_markdown(&title, &anns));
    }

    let mut stmt = conn.prepare(
        "SELECT a.id, a.blake3_hash, a.page_ord, a.kind, a.color, a.rects_json,
                a.quoted_text, a.note_markdown, a.created_at, a.updated_at,
                COALESCE((SELECT d.title FROM files f JOIN documents d ON d.id = f.document_id WHERE f.blake3_hash = a.blake3_hash LIMIT 1), a.blake3_hash)
         FROM file_annotations a
         ORDER BY a.blake3_hash ASC, a.page_ord ASC, a.created_at ASC",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(10)?, row_to_annotation(r)?)))?;
    let mut grouped: BTreeMap<String, Vec<FileAnnotation>> = BTreeMap::new();
    for r in rows {
        let (title, ann) = r?;
        grouped.entry(title).or_default().push(ann);
    }

    let mut sections = Vec::with_capacity(grouped.len());
    for (title, anns) in grouped {
        sections.push(format_doc_annotations_markdown(&title, &anns));
    }
    Ok(sections.join("\n\n---\n\n"))
}

fn format_doc_annotations_markdown(title: &str, anns: &[FileAnnotation]) -> String {
    let mut out = String::new();
    out.push_str("# ");
    out.push_str(title.trim());
    out.push('\n');
    if anns.is_empty() {
        out.push_str("\n_Nessuna annotazione._\n");
        return out;
    }
    for a in anns {
        out.push('\n');
        let kind_label = match a.kind.as_str() {
            "bookmark" => "Segnalibro",
            "note" => "Nota",
            _ => "Evidenziazione",
        };
        out.push_str(&format!("### {kind_label} — p. {}\n", a.page_ord));
        if !a.quoted_text.trim().is_empty() {
            for line in a.quoted_text.trim().lines() {
                out.push_str("> ");
                out.push_str(line);
                out.push('\n');
            }
            if !a.note_markdown.trim().is_empty() {
                out.push('\n');
            }
        }
        if !a.note_markdown.trim().is_empty() {
            out.push_str(a.note_markdown.trim());
            out.push('\n');
        }
    }
    out
}

pub fn get_reading_state_for_hash(
    conn: &Connection,
    blake3_hash: &str,
) -> Result<Option<FileReadingState>> {
    conn.query_row(
        "SELECT blake3_hash, last_page_ord, last_opened_at, zoom_mode, custom_zoom, rotation_deg, updated_at
         FROM file_reading_state
         WHERE blake3_hash = ?1",
        params![blake3_hash],
        |r| {
            Ok(FileReadingState {
                blake3_hash: r.get(0)?,
                last_page_ord: r.get::<_, i64>(1)?.max(1) as u32,
                last_opened_at: r.get(2)?,
                zoom_mode: r.get(3)?,
                custom_zoom: r.get::<_, f64>(4)? as f32,
                rotation_deg: normalize_rotation(r.get::<_, i64>(5)? as i32),
                updated_at: r.get(6)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_reading_state_for_file(
    conn: &Connection,
    file_id: i64,
) -> Result<Option<FileReadingState>> {
    let hash = lookup_file_hash(conn, file_id)?;
    get_reading_state_for_hash(conn, &hash)
}

pub fn upsert_reading_state_for_file(
    conn: &Connection,
    file_id: i64,
    patch: &ReadingStatePatch,
) -> Result<FileReadingState> {
    let hash = lookup_file_hash(conn, file_id)?;
    let existing = get_reading_state_for_hash(conn, &hash)?;
    let now = chrono::Utc::now().timestamp();

    let last_page_ord = patch
        .last_page_ord
        .map(|p| p.max(1))
        .or_else(|| existing.as_ref().map(|e| e.last_page_ord))
        .unwrap_or(1);
    let last_opened_at = if patch.touch_opened {
        Some(now)
    } else {
        existing
            .as_ref()
            .and_then(|e| e.last_opened_at)
            .or(Some(now))
    };
    let zoom_mode = patch
        .zoom_mode
        .as_deref()
        .map(normalize_zoom_mode)
        .or_else(|| existing.as_ref().map(|e| e.zoom_mode.as_str()))
        .unwrap_or("fit-width")
        .to_string();
    let custom_zoom = patch
        .custom_zoom
        .map(|z| z.clamp(0.35, 4.0))
        .or_else(|| existing.as_ref().map(|e| e.custom_zoom))
        .unwrap_or(1.0);
    let rotation_deg = patch
        .rotation_deg
        .map(normalize_rotation)
        .or_else(|| existing.as_ref().map(|e| e.rotation_deg))
        .unwrap_or(0);

    conn.execute(
        "INSERT INTO file_reading_state (
            blake3_hash, last_page_ord, last_opened_at, zoom_mode, custom_zoom, rotation_deg, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(blake3_hash) DO UPDATE SET
            last_page_ord = excluded.last_page_ord,
            last_opened_at = COALESCE(excluded.last_opened_at, file_reading_state.last_opened_at),
            zoom_mode = excluded.zoom_mode,
            custom_zoom = excluded.custom_zoom,
            rotation_deg = excluded.rotation_deg,
            updated_at = excluded.updated_at",
        params![
            hash,
            last_page_ord as i64,
            last_opened_at,
            zoom_mode,
            custom_zoom as f64,
            rotation_deg as i64,
            now
        ],
    )?;

    Ok(FileReadingState {
        blake3_hash: hash,
        last_page_ord,
        last_opened_at,
        zoom_mode,
        custom_zoom,
        rotation_deg,
        updated_at: now,
    })
}

pub fn set_document_reading_status(conn: &Connection, document_id: i64, status: &str) -> Result<()> {
    let norm = normalize_reading_status(status);
    let now = chrono::Utc::now().timestamp();
    let updated = conn.execute(
        "UPDATE documents SET reading_status = ?1, updated_at = ?2 WHERE id = ?3",
        params![norm, now, document_id],
    )?;
    if updated == 0 {
        return Err(CoreError::NotFound(format!("document {document_id}")));
    }
    Ok(())
}

pub fn set_document_starred(conn: &Connection, document_id: i64, starred: bool) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let updated = conn.execute(
        "UPDATE documents SET is_starred = ?1, updated_at = ?2 WHERE id = ?3",
        params![starred as i64, now, document_id],
    )?;
    if updated == 0 {
        return Err(CoreError::NotFound(format!("document {document_id}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::docs::{create_heuristic_document, insert_file_record, update_file_location, NewFileRecord};
    use crate::db::repos::roots::{insert_source_root, NewSourceRoot};
    use crate::db::Database;

    fn setup_db_with_file() -> (Database, i64, i64) {
        let db = Database::in_memory().unwrap();
        let w = db.writer();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: "r1".into(),
                label: "Math".into(),
                root_path: "/tmp/math".into(),
                is_network_or_cloud: false,
            },
        )
        .unwrap();
        let doc_id = create_heuristic_document(&w, "Poincaré Duality").unwrap();
        let file_id = insert_file_record(
            &w,
            &NewFileRecord {
                document_id: doc_id,
                root_id: "r1",
                rel_path: "poincare.pdf",
                format: "pdf",
                size_bytes: 2048,
                mtime_ns: 1,
                blake3_hash: "hash_poincare_001",
                index_state: "ready",
            },
        )
        .unwrap();
        drop(w);
        (db, doc_id, file_id)
    }

    #[test]
    fn annotations_survive_file_rename_search_with_diacritics_and_export_markdown() {
        let (db, _doc_id, file_id) = setup_db_with_file();
        let w = db.writer();

        let h = create_annotation_for_file(
            &w,
            file_id,
            &NewFileAnnotation {
                page_ord: 12,
                kind: "highlight".into(),
                color: "yellow".into(),
                rects: vec![NormalizedRect {
                    x: 0.1,
                    y: 0.25,
                    w: 0.7,
                    h: 0.04,
                }],
                quoted_text: "By Poincaré duality and Erdős theorem".into(),
                note_markdown: "Verify $H^k(M) \\cong H_{n-k}(M)$.".into(),
            },
        )
        .unwrap();
        assert_eq!(h.page_ord, 12);
        assert_eq!(h.rects.len(), 1);

        let bm = toggle_page_bookmark(&w, file_id, 42, Some("Main theorem")).unwrap();
        assert!(bm.is_some());

        // Move/rename file: blake3_hash stays identical -> annotations remain attached!
        update_file_location(&w, file_id, "r1", "topology/poincare_renamed.pdf", 2048, 2).unwrap();
        let listed = list_annotations_for_file(&w, file_id).unwrap();
        assert_eq!(listed.len(), 2);

        // Diacritic-tolerant search
        let hits = search_annotations(&w, "Poincare Erdos", Some(file_id)).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].annotation.id, h.id);

        // Markdown export
        let md = export_annotations_markdown(&w, Some(file_id)).unwrap();
        assert!(md.contains("# Poincaré Duality"));
        assert!(md.contains("> By Poincaré duality and Erdős theorem"));
        assert!(md.contains("$H^k(M) \\cong H_{n-k}(M)$"));
        assert!(md.contains("Segnalibro — p. 42"));

        // Toggle bookmark again removes it
        let removed = toggle_page_bookmark(&w, file_id, 42, None).unwrap();
        assert!(removed.is_none());
        assert_eq!(list_annotations_for_file(&w, file_id).unwrap().len(), 1);
    }

    #[test]
    fn reading_state_and_status_persist_cleanly() {
        let (db, doc_id, file_id) = setup_db_with_file();
        let w = db.writer();

        let st = upsert_reading_state_for_file(
            &w,
            file_id,
            &ReadingStatePatch {
                last_page_ord: Some(77),
                zoom_mode: Some("custom".into()),
                custom_zoom: Some(1.35),
                rotation_deg: Some(90),
                touch_opened: true,
            },
        )
        .unwrap();
        assert_eq!(st.last_page_ord, 77);
        assert_eq!(st.zoom_mode, "custom");
        assert_eq!(st.rotation_deg, 90);
        assert!(st.last_opened_at.is_some());

        set_document_reading_status(&w, doc_id, "reading").unwrap();
        set_document_starred(&w, doc_id, true).unwrap();

        let (rs, star): (String, i64) = w
            .query_row(
                "SELECT reading_status, is_starred FROM documents WHERE id = ?1",
                params![doc_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(rs, "reading");
        assert_eq!(star, 1);
    }
}
