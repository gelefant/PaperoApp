use crate::db::repos::docs::{enqueue_index_job, get_file_location};
use crate::error::{CoreError, Result};
use crate::extract::chunker::chunk_document;
use crate::extract::types::{ExtractedDocument, ExtractedPage, ExtractedSpan, NormalizedRect};
use crate::index::bm25::{Bm25Index, Bm25Passage};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AltFormulaImportReport {
    pub file_id: i64,
    pub document_id: i64,
    pub cache_path: PathBuf,
    pub passages_inserted: usize,
    pub pages_parsed: usize,
}

/// Imports a Nougat/Marker-style Markdown/LaTeX (`.mmd` / `.md` / `.tex`) transcript
/// for `file_id`, saves a local copy under `<cache_dir>/alt_text/`, inserts
/// formula passages (`is_alt_formula = 1`), indexes them into Tantivy BM25, and
/// queues background vector embedding without ever touching the user's source folder.
pub fn import_alt_formula_markdown(
    conn: &Connection,
    bm25: &Bm25Index,
    cache_dir: &Path,
    file_id: i64,
    markdown_text: &str,
    chunk_target_tokens: usize,
    chunk_overlap_tokens: usize,
) -> Result<AltFormulaImportReport> {
    let loc = get_file_location(conn, file_id)?
        .ok_or_else(|| CoreError::NotFound(format!("file {file_id}")))?;

    let alt_dir = cache_dir.join("alt_text");
    fs::create_dir_all(&alt_dir)?;
    let cache_path = alt_dir.join(format!("file_{file_id}.mmd"));
    fs::write(&cache_path, markdown_text)?;

    let pages = split_markdown_into_pages(markdown_text);
    let pages_len = pages.len();
    let doc = ExtractedDocument {
        page_count: pages_len as u32,
        has_text_layer: true,
        pages,
        outline: Vec::new(),
        xmp_xml: None,
        doc_title: None,
        doc_author: None,
    };
    let chunks = chunk_document(&doc, chunk_target_tokens, chunk_overlap_tokens);

    let old_alt_pids = get_alt_formula_passage_ids(conn, file_id)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM passages WHERE file_id = ?1 AND is_alt_formula = 1",
        params![file_id],
    )?;

    let mut inserted_bm25 = Vec::with_capacity(chunks.len());
    {
        let mut stmt = tx.prepare_cached(
            "INSERT INTO passages (
                document_id, file_id, page_ord, printed_label, chunk_index,
                is_alt_formula, token_count, text_content, bboxes_blob
             ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8)",
        )?;
        for ch in &chunks {
            stmt.execute(params![
                loc.document_id,
                file_id,
                ch.page_ord as i64,
                ch.printed_label,
                (10_000 + ch.chunk_index) as i64,
                ch.token_count as i64,
                ch.text_content,
                Vec::<u8>::new(),
            ])?;
            let pid = tx.last_insert_rowid() as u64;
            inserted_bm25.push(Bm25Passage {
                passage_id: pid,
                document_id: loc.document_id as u64,
                file_id: file_id as u64,
                root_id: loc.root_id.clone(),
                text_content: ch.text_content.clone(),
            });
        }
    }

    tx.execute(
        "UPDATE files SET embedded_light = 0, embedded_quality = 0 WHERE id = ?1",
        params![file_id],
    )?;
    enqueue_index_job(&tx, file_id, "embed_light", 25)?;
    tx.commit()?;

    if !old_alt_pids.is_empty() {
        bm25.delete_passages(&old_alt_pids)?;
    }
    bm25.add_passages(&inserted_bm25)?;
    bm25.commit()?;

    Ok(AltFormulaImportReport {
        file_id,
        document_id: loc.document_id,
        cache_path,
        passages_inserted: inserted_bm25.len(),
        pages_parsed: pages_len,
    })
}

/// Invokes a user-configured external Markdown/LaTeX OCR tool (`nougat` or `marker_single`)
/// writing ONLY into `<cache_dir>/alt_text/`, then imports the generated Markdown.
pub fn run_external_alt_formula_cli(
    conn: &Connection,
    bm25: &Bm25Index,
    cache_dir: &Path,
    file_id: i64,
    tool_cmd: &str,
    chunk_target_tokens: usize,
    chunk_overlap_tokens: usize,
) -> Result<AltFormulaImportReport> {
    let loc = get_file_location(conn, file_id)?
        .ok_or_else(|| CoreError::NotFound(format!("file {file_id}")))?;
    if !loc.abs_path.exists() {
        return Err(CoreError::NotFound(format!(
            "document file missing on disk: {}",
            loc.abs_path.display()
        )));
    }
    let work_out = cache_dir.join("alt_text").join(format!("run_{file_id}"));
    fs::create_dir_all(&work_out)?;

    let status = Command::new(tool_cmd)
        .arg(&loc.abs_path)
        .arg("-o")
        .arg(&work_out)
        .status()
        .map_err(|e| CoreError::MissingExternalTool {
            tool: tool_cmd.to_string(),
            detail: e.to_string(),
        })?;
    if !status.success() {
        return Err(CoreError::Extract(format!(
            "external OCR CLI '{tool_cmd}' failed with status {status}"
        )));
    }

    let generated = find_generated_markdown(&work_out).ok_or_else(|| {
        CoreError::Extract(format!(
            "no .mmd/.md output produced by '{tool_cmd}' in {}",
            work_out.display()
        ))
    })?;
    let text = fs::read_to_string(&generated)?;
    import_alt_formula_markdown(
        conn,
        bm25,
        cache_dir,
        file_id,
        &text,
        chunk_target_tokens,
        chunk_overlap_tokens,
    )
}

fn find_generated_markdown(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() {
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "mmd" | "md" | "tex") {
                return Some(p);
            }
        } else if p.is_dir() {
            if let Some(found) = find_generated_markdown(&p) {
                return Some(found);
            }
        }
    }
    None
}

fn split_markdown_into_pages(markdown: &str) -> Vec<ExtractedPage> {
    let raw_pages: Vec<&str> = if markdown.contains('\x0c') {
        markdown.split('\x0c').collect()
    } else {
        vec![markdown]
    };

    raw_pages
        .into_iter()
        .enumerate()
        .filter_map(|(idx, raw)| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            let page_ord = (idx + 1) as u32;
            Some(ExtractedPage {
                page_ord,
                printed_label: Some(page_ord.to_string()),
                width_pts: 612.0,
                height_pts: 792.0,
                raw_text: trimmed.to_string(),
                spans: vec![ExtractedSpan {
                    text: trimmed.to_string(),
                    bbox: NormalizedRect {
                        x: 0.0,
                        y: 0.0,
                        w: 1.0,
                        h: 1.0,
                    },
                }],
            })
        })
        .collect()
}

fn get_alt_formula_passage_ids(conn: &Connection, file_id: i64) -> Result<Vec<u64>> {
    let mut stmt =
        conn.prepare("SELECT id FROM passages WHERE file_id = ?1 AND is_alt_formula = 1")?;
    let rows = stmt.query_map(params![file_id], |r| r.get::<_, i64>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r? as u64);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repos::docs::{create_heuristic_document, insert_file_record, NewFileRecord};
    use crate::db::repos::roots::{insert_source_root, NewSourceRoot};
    use crate::db::Database;
    use tempfile::tempdir;

    #[test]
    fn imports_alt_formula_latex_and_indexes_in_bm25_and_cache() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("papero.db"), 2).unwrap();
        let bm25 = Bm25Index::open_or_create(&dir.path().join("tantivy")).unwrap();
        let root_dir = dir.path().join("library");
        fs::create_dir_all(&root_dir).unwrap();

        let w = db.writer();
        let root_id = "r1".to_string();
        insert_source_root(
            &w,
            &NewSourceRoot {
                id: root_id.clone(),
                label: "Math".into(),
                root_path: root_dir.to_string_lossy().into_owned(),
                is_network_or_cloud: false,
            },
        )
        .unwrap();
        let doc_id = create_heuristic_document(&w, "Riemann Zeta").unwrap();
        let file_id = insert_file_record(
            &w,
            &NewFileRecord {
                document_id: doc_id,
                root_id: &root_id,
                rel_path: "zeta.pdf",
                format: "pdf",
                size_bytes: 42,
                mtime_ns: 0,
                blake3_hash: "zeta-hash",
                index_state: "bm25_done",
            },
        )
        .unwrap();

        let md = "By analytic continuation:\n\n$$\\zeta(s) = \\sum_{n=1}^{\\infty} n^{-s}$$";
        let report = import_alt_formula_markdown(
            &w,
            &bm25,
            &dir.path().join("cache"),
            file_id,
            md,
            400,
            60,
        )
        .unwrap();

        assert_eq!(report.passages_inserted, 1);
        assert!(report.cache_path.exists());
        assert!(!root_dir.join("zeta.mmd").exists());

        let hits = bm25.search("analytic continuation", None, 5).unwrap();
        assert_eq!(hits.len(), 1);
    }
}
