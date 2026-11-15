use crate::config::{resolve_external_tool, AppConfig};
use crate::db::repos::docs::{
    enqueue_index_job, get_file_location, get_file_non_alt_passage_ids,
    replace_file_ocr_pages_and_passages, set_file_ocr_status, FilePageInsert, PassageInsert,
};
use crate::db::Database;
use crate::dedup::{compute_minhash, pack_minhash};
use crate::error::{CoreError, Result};
use crate::extract::chunker::chunk_document;
use crate::extract::pdf::bind_pdfium;
use crate::extract::types::{
    ExtractedDocument, ExtractedPage, ExtractedSpan, NormalizedRect,
};
use crate::fs::scanner::ScanControl;
use crate::index::bm25::{Bm25Index, Bm25Passage};
use crate::index::vector::VectorIndex;
use crate::metadata::extract::extract_identifiers_and_heuristics_with_front_pages;
use crate::metadata::resolver::apply_extracted_identifiers_to_doc;
use crate::ml::ModelProfile;
use pdfium_render::prelude::*;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const DEFAULT_PREFERRED_LANGS: &[&str] = &["eng", "ita", "fra", "deu"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrEnvironmentStatus {
    pub available: bool,
    pub tesseract_path: String,
    pub version: Option<String>,
    pub installed_langs: Vec<String>,
    pub selected_langs: Vec<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrFileReport {
    pub file_id: i64,
    pub document_id: i64,
    pub pages_processed: u32,
    pub passages_indexed: usize,
    pub languages_used: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrProgressEvent {
    pub file_id: i64,
    pub page_ord: u32,
    pub total_pages: u32,
}

struct TempOcrFile {
    path: PathBuf,
}

impl TempOcrFile {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TempOcrFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Detects external `tesseract` CLI availability (`--version` and `--list-langs`),
/// never failing when Tesseract is absent so the rest of Papero keeps running cleanly.
pub fn detect_tesseract(
    explicit_bin: Option<&str>,
    preferred_langs: Option<&str>,
) -> OcrEnvironmentStatus {
    let bin = resolve_external_tool(explicit_bin, "tesseract");

    let ver_out = match Command::new(&bin)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            return OcrEnvironmentStatus {
                available: false,
                tesseract_path: bin,
                version: None,
                installed_langs: Vec::new(),
                selected_langs: Vec::new(),
                reason: Some(format!(
                    "Tesseract non trovato ({e}). Installalo seguendo le istruzioni nel README."
                )),
            };
        }
    };

    if !ver_out.status.success() {
        let stderr = String::from_utf8_lossy(&ver_out.stderr).trim().to_string();
        return OcrEnvironmentStatus {
            available: false,
            tesseract_path: bin,
            version: None,
            installed_langs: Vec::new(),
            selected_langs: Vec::new(),
            reason: Some(format!("tesseract --version fallito: {stderr}")),
        };
    }

    let version_text = String::from_utf8_lossy(&ver_out.stdout);
    let stderr_ver = String::from_utf8_lossy(&ver_out.stderr);
    let version = version_text
        .lines()
        .chain(stderr_ver.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|s| s.to_string());

    let installed_langs = query_installed_languages(&bin);
    let selected_langs = select_ocr_languages(&installed_langs, preferred_langs);

    OcrEnvironmentStatus {
        available: true,
        tesseract_path: bin,
        version,
        installed_langs,
        selected_langs,
        reason: None,
    }
}

fn query_installed_languages(bin: &str) -> Vec<String> {
    let Ok(out) = Command::new(bin)
        .arg("--list-langs")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut langs = Vec::new();
    for line in stdout.lines().chain(stderr.lines()) {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("List of available languages")
            || trimmed.contains('/')
            || trimmed.contains(' ')
        {
            continue;
        }
        if !langs.iter().any(|l| l == trimmed) {
            langs.push(trimmed.to_string());
        }
    }
    langs
}

/// Selects the active Tesseract language packs from `installed_langs` matching either
/// an explicit expression (e.g. `"eng+ita+rus"`) or the default `eng+ita+fra+deu` set.
pub fn select_ocr_languages(installed_langs: &[String], preferred: Option<&str>) -> Vec<String> {
    if let Some(pref) = preferred.map(str::trim).filter(|s| !s.is_empty()) {
        let mut chosen = Vec::new();
        for tok in pref.split(['+', ',', ' ']).map(str::trim).filter(|s| !s.is_empty()) {
            if (installed_langs.is_empty() || installed_langs.iter().any(|l| l == tok))
                && !chosen.iter().any(|c| c == tok)
            {
                chosen.push(tok.to_string());
            }
        }
        if !chosen.is_empty() {
            return chosen;
        }
    }

    let mut defaults: Vec<String> = DEFAULT_PREFERRED_LANGS
        .iter()
        .filter(|&&code| installed_langs.iter().any(|l| l == code))
        .map(|&s| s.to_string())
        .collect();
    if defaults.is_empty() {
        if let Some(first) = installed_langs
            .iter()
            .find(|l| l.as_str() != "osd" && l.as_str() != "equ")
            .or_else(|| installed_langs.first())
        {
            defaults.push(first.clone());
        }
    }
    defaults
}

/// Parses Tesseract TSV output (`level == 1` page size, `level == 5` words) into
/// line-grouped [`ExtractedSpan`]s with TOP-LEFT normalized bounding boxes in `[0.0, 1.0]`.
pub fn parse_tesseract_tsv(
    tsv: &str,
    page_ord: u32,
    width_pts: f32,
    height_pts: f32,
    printed_label: Option<String>,
) -> Result<ExtractedPage> {
    let mut page_w_px: f32 = 0.0;
    let mut page_h_px: f32 = 0.0;
    let mut max_right_px: f32 = 0.0;
    let mut max_bottom_px: f32 = 0.0;

    type LineKey = (u32, u32, u32);
    type TsvWordBox = (String, f32, f32, f32, f32);
    let mut lines_map: BTreeMap<LineKey, Vec<TsvWordBox>> = BTreeMap::new();

    for (idx, line) in tsv.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if idx == 0 && cols.first().map(|s| s.trim()) == Some("level") {
            continue;
        }
        if cols.len() < 11 {
            continue;
        }

        let Ok(level) = cols[0].trim().parse::<u32>() else {
            continue;
        };
        let Ok(block_num) = cols[2].trim().parse::<u32>() else {
            continue;
        };
        let Ok(par_num) = cols[3].trim().parse::<u32>() else {
            continue;
        };
        let Ok(line_num) = cols[4].trim().parse::<u32>() else {
            continue;
        };
        let left = cols[6].trim().parse::<f32>().unwrap_or(0.0);
        let top = cols[7].trim().parse::<f32>().unwrap_or(0.0);
        let width = cols[8].trim().parse::<f32>().unwrap_or(0.0);
        let height = cols[9].trim().parse::<f32>().unwrap_or(0.0);
        let conf = cols[10].trim().parse::<f32>().unwrap_or(-1.0);
        let text = cols.get(11).copied().unwrap_or("").trim();

        max_right_px = max_right_px.max(left + width);
        max_bottom_px = max_bottom_px.max(top + height);

        if level == 1 {
            if width > 0.0 && height > 0.0 {
                page_w_px = width;
                page_h_px = height;
            }
            continue;
        }

        if level == 5 && conf >= 0.0 && !text.is_empty() && width > 0.0 && height > 0.0 {
            lines_map
                .entry((block_num, par_num, line_num))
                .or_default()
                .push((text.to_string(), left, top, width, height));
        }
    }

    if page_w_px <= 0.0 {
        page_w_px = max_right_px.max(1.0);
    }
    if page_h_px <= 0.0 {
        page_h_px = max_bottom_px.max(1.0);
    }

    let eff_width_pts = if width_pts > 0.0 { width_pts } else { page_w_px };
    let eff_height_pts = if height_pts > 0.0 { height_pts } else { page_h_px };

    let mut spans = Vec::with_capacity(lines_map.len());
    let mut raw_lines = Vec::with_capacity(lines_map.len());

    for words in lines_map.into_values() {
        let mut line_text = String::new();
        let mut line_bbox: Option<NormalizedRect> = None;

        for (w_text, left, top, w, h) in words {
            if !line_text.is_empty() {
                line_text.push(' ');
            }
            line_text.push_str(&w_text);

            let rect = NormalizedRect {
                x: (left / page_w_px).clamp(0.0, 1.0),
                y: (top / page_h_px).clamp(0.0, 1.0),
                w: (w / page_w_px).clamp(0.0, 1.0),
                h: (h / page_h_px).clamp(0.0, 1.0),
            };
            line_bbox = Some(match line_bbox {
                Some(existing) => existing.union(&rect),
                None => rect,
            });
        }

        let trimmed = line_text.trim();
        if !trimmed.is_empty() {
            if let Some(bbox) = line_bbox {
                spans.push(ExtractedSpan {
                    text: trimmed.to_string(),
                    bbox,
                });
                raw_lines.push(trimmed.to_string());
            }
        }
    }

    Ok(ExtractedPage {
        page_ord,
        printed_label,
        width_pts: eff_width_pts,
        height_pts: eff_height_pts,
        spans,
        raw_text: raw_lines.join("\n"),
    })
}

/// Runs external Tesseract OCR page-by-page over a PDF or DjVu file, rendering temporary
/// raster images exclusively inside `<cache_dir>/ocr_tmp/` and updating SQLite + Tantivy BM25
/// (and cleaning stale keys from `VectorIndex` if opened) while preserving source files read-only.
#[allow(clippy::too_many_arguments)]
pub fn run_ocr_for_file<F>(
    db: &Database,
    bm25: &Bm25Index,
    vector_index: Option<&VectorIndex>,
    cache_dir: &Path,
    config: &AppConfig,
    file_id: i64,
    control: Option<&ScanControl>,
    mut progress_cb: F,
) -> Result<OcrFileReport>
where
    F: FnMut(OcrProgressEvent),
{
    let env = detect_tesseract(
        config.tesseract_bin.as_deref(),
        config.ocr_languages.as_deref(),
    );
    if !env.available {
        return Err(CoreError::MissingExternalTool {
            tool: "tesseract".to_string(),
            detail: env
                .reason
                .unwrap_or_else(|| "tesseract not available".to_string()),
        });
    }

    let loc = {
        let r = db.reader();
        get_file_location(&r, file_id)?
    }
    .ok_or_else(|| CoreError::NotFound(format!("file {file_id}")))?;

    if !loc.is_available || !loc.abs_path.exists() {
        return Err(CoreError::NotFound(format!(
            "source file unavailable on disk: {}",
            loc.abs_path.display()
        )));
    }

    {
        let w = db.writer();
        set_file_ocr_status(&w, file_id, "running", None)?;
    }

    let lang_arg = env.selected_langs.join("+");
    let dpi = config.ocr_dpi.clamp(150, 350);
    let ocr_tmp_dir = cache_dir.join("ocr_tmp");
    fs::create_dir_all(&ocr_tmp_dir)?;

    let extract_res = match loc.format.as_str() {
        "pdf" => run_ocr_pdf(
            &loc.abs_path,
            file_id,
            &env.tesseract_path,
            &lang_arg,
            dpi,
            &ocr_tmp_dir,
            config.pdfium_lib_path.as_deref(),
            control,
            &mut progress_cb,
        ),
        "djvu" => run_ocr_djvu(
            &loc.abs_path,
            file_id,
            &env.tesseract_path,
            config.djvused_bin.as_deref(),
            config.ddjvu_bin.as_deref(),
            &lang_arg,
            dpi,
            &ocr_tmp_dir,
            control,
            &mut progress_cb,
        ),
        other => Err(CoreError::CorruptFile(format!(
            "unsupported format for OCR: {other}"
        ))),
    };

    let pages = match extract_res {
        Ok(p) => p,
        Err(e) => {
            let w = db.writer();
            let _ = set_file_ocr_status(&w, file_id, "failed", Some(&e.to_string()));
            return Err(e);
        }
    };

    let page_count = pages.len() as u32;
    let doc = ExtractedDocument {
        page_count,
        has_text_layer: true,
        pages,
        outline: Vec::new(),
        xmp_xml: None,
        doc_title: None,
        doc_author: None,
    };
    let built_chunks = chunk_document(&doc, config.chunk_target_tokens, config.chunk_overlap_tokens);

    let page_inserts: Vec<FilePageInsert> = doc
        .pages
        .iter()
        .map(|p| FilePageInsert {
            file_id,
            page_ord: p.page_ord,
            printed_label: p.printed_label.clone(),
            width_pts: p.width_pts,
            height_pts: p.height_pts,
            text_source: "ocr".to_string(),
        })
        .collect();

    let passage_inserts: Vec<PassageInsert> = built_chunks
        .iter()
        .map(|c| PassageInsert {
            document_id: loc.document_id,
            file_id,
            page_ord: c.page_ord,
            printed_label: c.printed_label.clone(),
            chunk_index: c.chunk_index,
            is_alt_formula: false,
            token_count: c.token_count,
            text_content: c.text_content.clone(),
            bboxes_blob: c.bboxes_blob.clone(),
        })
        .collect();

    let (old_pids, new_pids) = {
        let w = db.writer();
        let old_pids = get_file_non_alt_passage_ids(&w, file_id)?;
        let new_pids = replace_file_ocr_pages_and_passages(
            &w,
            file_id,
            page_count,
            &page_inserts,
            &passage_inserts,
        )?;

        let first_pages: Vec<&str> = doc
            .pages
            .iter()
            .take(4)
            .map(|p| p.raw_text.as_str())
            .collect();
        let last_page = doc.pages.last().map(|p| p.raw_text.as_str());
        let stem = Path::new(&loc.rel_path)
            .file_stem()
            .and_then(|s| s.to_str());
        let ids = extract_identifiers_and_heuristics_with_front_pages(
            None,
            None,
            None,
            &first_pages,
            last_page,
            page_count,
            stem,
            doc.pages.first(),
            doc.pages.get(1),
        );
        let _ = apply_extracted_identifiers_to_doc(&w, loc.document_id, &ids, None);

        let full_text_for_minhash = doc
            .pages
            .iter()
            .take(15)
            .map(|p| p.raw_text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(sig) = compute_minhash(&full_text_for_minhash) {
            let blob = pack_minhash(&sig);
            let _ = w.execute(
                "UPDATE documents SET minhash_sig = ?1 WHERE id = ?2",
                params![blob, loc.document_id],
            );
        }

        let embed_stage = match ModelProfile::from_str_lossy(&config.ml_profile) {
            ModelProfile::Light => "embed_light",
            ModelProfile::Quality => "embed_quality",
        };
        let _ = enqueue_index_job(&w, file_id, "metadata", 20);
        let _ = enqueue_index_job(&w, file_id, embed_stage, 30);

        (old_pids, new_pids)
    };

    if !old_pids.is_empty() {
        bm25.delete_passages(&old_pids)?;
        if let Some(vec_idx) = vector_index {
            let _ = vec_idx.remove_keys(&old_pids);
        }
    }

    let bm25_batch: Vec<Bm25Passage> = new_pids
        .into_iter()
        .zip(passage_inserts)
        .map(|(pid, p)| Bm25Passage {
            passage_id: pid,
            document_id: loc.document_id as u64,
            file_id: file_id as u64,
            root_id: loc.root_id.clone(),
            text_content: p.text_content,
        })
        .collect();

    let passages_indexed = bm25_batch.len();
    bm25.add_passages(&bm25_batch)?;
    bm25.commit()?;

    Ok(OcrFileReport {
        file_id,
        document_id: loc.document_id,
        pages_processed: page_count,
        passages_indexed,
        languages_used: lang_arg,
    })
}

/// Marks `file_id` as queued for background OCR (`ocr_status = 'queued'`) and adds an `ocr` job.
pub fn enqueue_ocr_for_file(conn: &Connection, file_id: i64) -> Result<()> {
    set_file_ocr_status(conn, file_id, "queued", None)?;
    enqueue_index_job(conn, file_id, "ocr", 25)?;
    Ok(())
}

/// Queues all reachable files lacking a native text layer (`has_text_layer = 0 AND ocr_status = 'none'`).
pub fn enqueue_ocr_for_scanned_files(conn: &Connection) -> Result<usize> {
    let mut stmt = conn.prepare(
        "SELECT f.id
         FROM files f
         JOIN source_roots sr ON sr.id = f.root_id
         WHERE sr.is_reachable = 1
           AND f.has_text_layer = 0
           AND f.ocr_status = 'none'
           AND f.index_state IN ('bm25_done', 'ready')
         ORDER BY f.id ASC",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
    let mut file_ids = Vec::new();
    for r in rows {
        file_ids.push(r?);
    }
    drop(stmt);

    for fid in &file_ids {
        enqueue_ocr_for_file(conn, *fid)?;
    }
    Ok(file_ids.len())
}

/// Runs OCR sequentially over all files currently marked `ocr_status = 'queued'`.
pub fn run_pending_ocr_queue<F>(
    db: &Database,
    bm25: &Bm25Index,
    vector_index: Option<&VectorIndex>,
    cache_dir: &Path,
    config: &AppConfig,
    control: Option<&ScanControl>,
    mut progress_cb: F,
) -> Result<Vec<OcrFileReport>>
where
    F: FnMut(OcrProgressEvent),
{
    let file_ids = {
        let r = db.reader();
        let mut stmt = r.prepare(
            "SELECT f.id
             FROM files f
             JOIN source_roots sr ON sr.id = f.root_id
             WHERE sr.is_reachable = 1 AND f.ocr_status = 'queued'
             ORDER BY f.id ASC",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
        let mut ids = Vec::new();
        for id in rows {
            ids.push(id?);
        }
        ids
    };

    let mut reports = Vec::new();
    for fid in file_ids {
        if let Some(ctrl) = control {
            if ctrl.wait_if_paused().is_err() || ctrl.is_cancelled() {
                break;
            }
        }
        if let Ok(rep) = run_ocr_for_file(
            db,
            bm25,
            vector_index,
            cache_dir,
            config,
            fid,
            control,
            &mut progress_cb,
        ) {
            let w = db.writer();
            let _ = w.execute(
                "DELETE FROM index_jobs WHERE file_id = ?1 AND stage = 'ocr'",
                params![fid],
            );
            reports.push(rep);
        }
    }
    Ok(reports)
}

#[allow(clippy::too_many_arguments)]
fn run_ocr_pdf<F>(
    pdf_path: &Path,
    file_id: i64,
    tesseract_bin: &str,
    lang_arg: &str,
    dpi: u32,
    ocr_tmp_dir: &Path,
    pdfium_lib_path: Option<&Path>,
    control: Option<&ScanControl>,
    progress_cb: &mut F,
) -> Result<Vec<ExtractedPage>>
where
    F: FnMut(OcrProgressEvent),
{
    let pdfium = bind_pdfium(pdfium_lib_path)?;
    let doc = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|e| CoreError::CorruptFile(format!("{}: {e:?}", pdf_path.display())))?;

    let total_pages = doc.pages().len() as u32;
    let mut pages = Vec::with_capacity(total_pages as usize);

    for (idx, page) in doc.pages().iter().enumerate() {
        if let Some(ctrl) = control {
            ctrl.wait_if_paused()?;
        }
        let page_ord = (idx as u32) + 1;
        progress_cb(OcrProgressEvent {
            file_id,
            page_ord,
            total_pages,
        });

        let width_pts = page.width().value;
        let height_pts = page.height().value;
        let printed_label = page
            .label()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let target_w = ((width_pts / 72.0) * (dpi as f32))
            .round()
            .clamp(300.0, 6000.0) as Pixels;
        let bitmap = page
            .render_with_config(&PdfRenderConfig::new().set_target_width(target_w))
            .map_err(|e| CoreError::Extract(format!("PDFium OCR raster p.{page_ord}: {e:?}")))?;

        let bw = bitmap.width() as u32;
        let bh = bitmap.height() as u32;
        let rgba = bitmap.as_rgba_bytes();

        let tmp_png_path = ocr_tmp_dir.join(format!("ocr_{file_id}_p{page_ord}.png"));
        let _guard = TempOcrFile::new(tmp_png_path.clone());
        write_rgba_png(&tmp_png_path, bw, bh, &rgba)?;

        let tsv = invoke_tesseract_tsv(tesseract_bin, &tmp_png_path, lang_arg, dpi)?;
        pages.push(parse_tesseract_tsv(
            &tsv,
            page_ord,
            width_pts,
            height_pts,
            printed_label,
        )?);
    }

    Ok(pages)
}

#[allow(clippy::too_many_arguments)]
fn run_ocr_djvu<F>(
    djvu_path: &Path,
    file_id: i64,
    tesseract_bin: &str,
    djvused_bin: Option<&str>,
    ddjvu_bin: Option<&str>,
    lang_arg: &str,
    dpi: u32,
    ocr_tmp_dir: &Path,
    control: Option<&ScanControl>,
    progress_cb: &mut F,
) -> Result<Vec<ExtractedPage>>
where
    F: FnMut(OcrProgressEvent),
{
    let djvused = resolve_external_tool(djvused_bin, "djvused");
    let ddjvu = resolve_external_tool(ddjvu_bin, "ddjvu");

    let count_out = Command::new(&djvused)
        .arg(djvu_path)
        .arg("-u")
        .arg("-e")
        .arg("n")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| CoreError::MissingExternalTool {
            tool: djvused.clone(),
            detail: e.to_string(),
        })?;
    if !count_out.status.success() {
        return Err(CoreError::CorruptFile(format!(
            "djvused failed on {}",
            djvu_path.display()
        )));
    }
    let total_pages: u32 = String::from_utf8_lossy(&count_out.stdout)
        .trim()
        .parse()
        .map_err(|_| CoreError::CorruptFile("invalid DjVu page count".into()))?;

    let mut pages = Vec::with_capacity(total_pages as usize);
    for page_ord in 1..=total_pages {
        if let Some(ctrl) = control {
            ctrl.wait_if_paused()?;
        }
        progress_cb(OcrProgressEvent {
            file_id,
            page_ord,
            total_pages,
        });

        let tmp_pnm_path = ocr_tmp_dir.join(format!("ocr_{file_id}_p{page_ord}.pnm"));
        let _guard = TempOcrFile::new(tmp_pnm_path.clone());

        let ddjvu_out = Command::new(&ddjvu)
            .arg("-format=pnm")
            .arg(format!("-page={page_ord}"))
            .arg(format!("-scale={dpi}"))
            .arg(djvu_path)
            .arg(&tmp_pnm_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| CoreError::MissingExternalTool {
                tool: ddjvu.clone(),
                detail: e.to_string(),
            })?;
        if !ddjvu_out.status.success() {
            let stderr = String::from_utf8_lossy(&ddjvu_out.stderr).trim().to_string();
            return Err(CoreError::Extract(format!(
                "ddjvu PNM render p.{page_ord} failed: {stderr}"
            )));
        }

        let tsv = invoke_tesseract_tsv(tesseract_bin, &tmp_pnm_path, lang_arg, dpi)?;
        pages.push(parse_tesseract_tsv(&tsv, page_ord, 0.0, 0.0, None)?);
    }

    Ok(pages)
}

fn write_rgba_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let file = fs::File::create(path)?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder
        .write_header()
        .map_err(|e| CoreError::Extract(format!("OCR PNG header error: {e}")))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| CoreError::Extract(format!("OCR PNG write error: {e}")))?;
    Ok(())
}

fn invoke_tesseract_tsv(
    tesseract_bin: &str,
    image_path: &Path,
    lang_arg: &str,
    dpi: u32,
) -> Result<String> {
    let mut cmd = Command::new(tesseract_bin);
    cmd.arg(image_path).arg("stdout");
    if !lang_arg.trim().is_empty() {
        cmd.arg("-l").arg(lang_arg);
    }
    cmd.arg("--dpi").arg(dpi.to_string()).arg("tsv");

    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| CoreError::MissingExternalTool {
            tool: tesseract_bin.to_string(),
            detail: format!("failed spawning tesseract: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(CoreError::Extract(format!(
            "tesseract exited with {}: {stderr}",
            output.status
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tesseract_tsv_top_left_line_spans() {
        let sample_tsv = "\
level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext
1\t1\t0\t0\t0\t0\t0\t0\t1000\t2000\t-1\t
2\t1\t1\t0\t0\t0\t100\t200\t700\t120\t-1\t
4\t1\t1\t1\t1\t0\t100\t200\t700\t40\t-1\t
5\t1\t1\t1\t1\t1\t100\t200\t180\t40\t96.5\tTheorem
5\t1\t1\t1\t1\t2\t300\t200\t120\t40\t95.1\t3.14
4\t1\t1\t1\t2\t0\t100\t300\t600\t40\t-1\t
5\t1\t1\t1\t2\t1\t100\t300\t200\t40\t94.0\tGalois
5\t1\t1\t1\t2\t2\t320\t300\t180\t40\t93.0\tTheory
";

        let page = parse_tesseract_tsv(sample_tsv, 1, 595.0, 842.0, Some("iv".into())).unwrap();
        assert_eq!(page.page_ord, 1);
        assert_eq!(page.printed_label.as_deref(), Some("iv"));
        assert_eq!(page.spans.len(), 2);
        assert_eq!(page.spans[0].text, "Theorem 3.14");
        assert_eq!(page.spans[1].text, "Galois Theory");
        assert_eq!(page.raw_text, "Theorem 3.14\nGalois Theory");

        // Top-left normalized coordinates: line 1 x=100..420/1000, y=200..240/2000
        assert!((page.spans[0].bbox.x - 0.10).abs() < 1e-5);
        assert!((page.spans[0].bbox.y - 0.10).abs() < 1e-5);
        assert!((page.spans[0].bbox.w - 0.32).abs() < 1e-5);
        assert!((page.spans[0].bbox.h - 0.02).abs() < 1e-5);
        // Line 2 has larger y (lower down from top)
        assert!(page.spans[1].bbox.y > page.spans[0].bbox.y);
    }

    #[test]
    fn selects_installed_ocr_languages_respecting_preference() {
        let installed = vec![
            "eng".to_string(),
            "ita".to_string(),
            "osd".to_string(),
            "rus".to_string(),
        ];
        let default_sel = select_ocr_languages(&installed, None);
        assert_eq!(default_sel, vec!["eng", "ita"]);

        let custom_sel = select_ocr_languages(&installed, Some("rus+ita+zzz"));
        assert_eq!(custom_sel, vec!["rus", "ita"]);

        let fallback_only = vec!["osd".to_string(), "jpn".to_string()];
        assert_eq!(select_ocr_languages(&fallback_only, None), vec!["jpn"]);
    }

    fn glyph_5x7(ch: char) -> [&'static str; 7] {
        match ch {
            'M' => ["10001", "11011", "10101", "10001", "10001", "10001", "10001"],
            'A' => ["01110", "10001", "10001", "11111", "10001", "10001", "10001"],
            'T' => ["11111", "00100", "00100", "00100", "00100", "00100", "00100"],
            'H' => ["10001", "10001", "10001", "11111", "10001", "10001", "10001"],
            _ => ["00000"; 7],
        }
    }

    fn render_banner_pbm(text: &str, scale: usize) -> Vec<u8> {
        let chars: Vec<char> = text.chars().collect();
        let pad = 16 * scale;
        let char_adv = 8 * scale;
        let width = pad * 2 + chars.len() * char_adv;
        let height = pad * 2 + 7 * scale;
        let mut out = format!("P1\n{width} {height}\n").into_bytes();

        for py in 0..height {
            for px in 0..width {
                let mut bit = '0';
                if py >= pad && py < pad + 7 * scale && px >= pad {
                    let gx = px - pad;
                    let idx = gx / char_adv;
                    let cx = (gx % char_adv) / scale;
                    let cy = (py - pad) / scale;
                    if idx < chars.len() && cx < 5 && cy < 7 {
                        let row = glyph_5x7(chars[idx])[cy];
                        if row.as_bytes()[cx] == b'1' {
                            bit = '1';
                        }
                    }
                }
                out.push(bit as u8);
                out.push(b' ');
            }
            out.push(b'\n');
        }
        out
    }

    #[test]
    fn ocr_end_to_end_djvu_through_ddjvu_and_tesseract() {
        let tess = detect_tesseract(None, None);
        if !tess.available
            || Command::new(resolve_external_tool(None, "cjb2"))
                .arg("--help")
                .output()
                .is_err()
            || Command::new(resolve_external_tool(None, "ddjvu"))
                .arg("--help")
                .output()
                .is_err()
        {
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().join("library");
        let app_dir = tmp.path().join("appdata");
        fs::create_dir_all(&root_dir).unwrap();
        fs::create_dir_all(&app_dir).unwrap();

        let pbm_path = tmp.path().join("scan.pbm");
        let djvu_path = root_dir.join("scanned_math.djvu");
        fs::write(&pbm_path, render_banner_pbm("MATH", 8)).unwrap();
        let cjb2_status = Command::new(resolve_external_tool(None, "cjb2"))
            .arg(&pbm_path)
            .arg(&djvu_path)
            .status()
            .unwrap();
        assert!(cjb2_status.success());

        let djvu_bytes_before = fs::read(&djvu_path).unwrap();

        let db = Database::open(&app_dir.join("papero.db"), 2).unwrap();
        let bm25 = Bm25Index::open_or_create(&app_dir.join("tantivy")).unwrap();
        let cfg = AppConfig {
            ocr_languages: tess.selected_langs.first().cloned(),
            ..Default::default()
        };
        {
            let w = db.writer();
            crate::db::repos::roots::insert_source_root(
                &w,
                &crate::db::repos::roots::NewSourceRoot {
                    id: "r1".into(),
                    label: "Library".into(),
                    root_path: root_dir.to_string_lossy().into_owned(),
                    is_network_or_cloud: false,
                },
            )
            .unwrap();
        }
        crate::fs::scanner::rescan_sources(&db, Some(&bm25), None, None).unwrap();
        let file_id: i64 = {
            let r = db.reader();
            r.query_row("SELECT id FROM files LIMIT 1", [], |row| row.get(0))
                .unwrap()
        };

        let mut progress_events = Vec::new();
        let report = run_ocr_for_file(
            &db,
            &bm25,
            None,
            &app_dir.join("cache"),
            &cfg,
            file_id,
            None,
            |ev| progress_events.push(ev),
        )
        .unwrap();

        assert_eq!(report.pages_processed, 1);
        assert_eq!(progress_events.len(), 1);
        // Source file remains byte-identical and strictly read-only.
        assert_eq!(fs::read(&djvu_path).unwrap(), djvu_bytes_before);

        let (has_text, ocr_status, text_source): (i64, String, String) = {
            let r = db.reader();
            r.query_row(
                "SELECT f.has_text_layer, f.ocr_status, fp.text_source
                 FROM files f
                 JOIN file_pages fp ON fp.file_id = f.id
                 WHERE f.id = ?1 LIMIT 1",
                params![file_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
        };
        assert_eq!(has_text, 1);
        assert_eq!(ocr_status, "done");
        assert_eq!(text_source, "ocr");
    }
}
