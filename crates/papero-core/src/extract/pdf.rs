use crate::config::bundled_resource_dirs;
use crate::error::{CoreError, Result};
use crate::extract::types::{
    ExtractedDocument, ExtractedPage, ExtractedSpan, NormalizedRect, OutlineItem,
};
use pdfium_render::prelude::*;
use std::ops::Deref;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

struct ExclusivePdfium(Pdfium);

// SAFETY: `ExclusivePdfium` is stored exclusively inside `PDFIUM_INSTANCE` and only ever accessed
// while holding its `MutexGuard`, enforcing strictly single-threaded access to PDFium's C++ library
// state and preventing `FPDF_DestroyLibrary` from tearing down global state across worker threads.
unsafe impl Send for ExclusivePdfium {}

static PDFIUM_INSTANCE: Mutex<Option<ExclusivePdfium>> = Mutex::new(None);

pub struct PdfiumGuard(MutexGuard<'static, Option<ExclusivePdfium>>);

impl Deref for PdfiumGuard {
    type Target = Pdfium;

    fn deref(&self) -> &Self::Target {
        &self.0.as_ref().expect("pdfium Mutex initialized").0
    }
}

/// Acquires exclusive access to the process-lifetime `Pdfium` instance so `FPDF_InitLibrary`
/// runs at most once and concurrent Rayon/Tokio threads never race inside PDFium C++ globals.
pub fn lock_cached_pdfium(explicit_path: Option<&Path>) -> Result<PdfiumGuard> {
    let mut guard = PDFIUM_INSTANCE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(ExclusivePdfium(bind_pdfium(explicit_path)?));
    }
    Ok(PdfiumGuard(guard))
}

/// Raw character + bottom-left point coordinates extracted from PDFium.
#[derive(Debug, Clone)]
pub struct PdfCharBox {
    pub ch: char,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
}

/// Binds PDFium dynamic library from explicit path, `PDFIUM_DYNAMIC_LIB_PATH`,
/// executable directory, or system library locations.
pub fn bind_pdfium(explicit_path: Option<&Path>) -> Result<Pdfium> {
    if let Some(p) = explicit_path {
        let bindings = if p.is_dir() {
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(p))
        } else {
            Pdfium::bind_to_library(p)
        }
        .map_err(|e| CoreError::MissingExternalTool {
            tool: "pdfium".to_string(),
            detail: format!("failed to load pdfium at {}: {e:?}", p.display()),
        })?;
        return Ok(Pdfium::new(bindings));
    }

    if let Ok(env_path) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
        let p = Path::new(&env_path);
        let bindings = if p.is_dir() {
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(p))
        } else {
            Pdfium::bind_to_library(p)
        }
        .map_err(|e| CoreError::MissingExternalTool {
            tool: "pdfium".to_string(),
            detail: format!("failed to load pdfium from PDFIUM_DYNAMIC_LIB_PATH ({env_path}): {e:?}"),
        })?;
        return Ok(Pdfium::new(bindings));
    }

    for dir in bundled_resource_dirs() {
        let lib_candidate = Pdfium::pdfium_platform_library_name_at_path(&dir);
        if lib_candidate.is_file() {
            if let Ok(bindings) = Pdfium::bind_to_library(lib_candidate) {
                return Ok(Pdfium::new(bindings));
            }
        }
    }

    let bindings = Pdfium::bind_to_system_library().map_err(|e| CoreError::MissingExternalTool {
        tool: "pdfium".to_string(),
        detail: format!("pdfium library not found on system or beside binary: {e:?}"),
    })?;
    Ok(Pdfium::new(bindings))
}

/// Extracts pages, labels, normalized top-left line bounding boxes, bookmarks, and metadata
/// from a PDF file using PDFium.
pub fn extract_pdf(path: &Path, pdfium_lib_path: Option<&Path>) -> Result<ExtractedDocument> {
    let pdfium = lock_cached_pdfium(pdfium_lib_path)?;
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|err| classify_pdfium_load_error(path, err))?;

    let page_count = doc.pages().len() as u32;
    let mut pages = Vec::with_capacity(page_count as usize);
    let mut has_text_layer = false;
    let mut page1_link_uris: Vec<String> = Vec::new();
    let mut char_boxes: Vec<PdfCharBox> = Vec::with_capacity(4096);

    for (idx, page) in doc.pages().iter().enumerate() {
        let page_ord = (idx as u32) + 1;
        let width_pts = page.width().value;
        let height_pts = page.height().value;
        let printed_label = page.label().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());

        if page_ord <= 2 {
            collect_page_identifier_links(&page, &mut page1_link_uris);
        }

        char_boxes.clear();
        let mut raw_text = String::new();
        if let Ok(text_page) = page.text() {
            raw_text = text_page.all();
            for ch in text_page.chars().iter() {
                if let Some(unicode) = ch.unicode_char() {
                    if let Ok(bounds) = ch.loose_bounds() {
                        char_boxes.push(PdfCharBox {
                            ch: unicode,
                            left: bounds.left().value,
                            bottom: bounds.bottom().value,
                            right: bounds.right().value,
                            top: bounds.top().value,
                        });
                    }
                }
            }
        }

        let spans = group_pdf_chars_into_lines(&char_boxes, width_pts, height_pts);
        if !raw_text.trim().is_empty() {
            has_text_layer = true;
        }

        pages.push(ExtractedPage {
            page_ord,
            printed_label,
            width_pts,
            height_pts,
            spans,
            raw_text,
        });
    }

    let outline = extract_bookmarks(&doc);
    let (doc_title, doc_author, mut extra_meta) = extract_pdf_metadata_fields(&doc);
    if !page1_link_uris.is_empty() {
        let joined_links = page1_link_uris.join(" \n ");
        extra_meta = Some(match extra_meta {
            Some(prev) => format!("{prev} \n {joined_links}"),
            None => joined_links,
        });
    }
    let xmp_xml = merge_xmp_and_extra_meta(extract_xmp_from_raw_pdf(path), extra_meta);

    Ok(ExtractedDocument {
        page_count,
        has_text_layer,
        pages,
        outline,
        xmp_xml,
        doc_title,
        doc_author,
    })
}

/// Fast front-matter extractor for bibliographic resolution: reads PDF metadata, XMP
/// (head + tail), page 1 with full character/line bounding boxes (`spans`), pages 2..=4
/// text, and the final page text without parsing the entire body of large documents.
pub fn extract_pdf_front_matter(
    path: &Path,
    pdfium_lib_path: Option<&Path>,
) -> Result<ExtractedDocument> {
    let pdfium = lock_cached_pdfium(pdfium_lib_path)?;
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|err| classify_pdfium_load_error(path, err))?;

    let page_count = doc.pages().len() as u32;
    let mut pages = Vec::new();
    let mut has_text_layer = false;
    let mut page1_link_uris: Vec<String> = Vec::new();

    let mut target_ords: Vec<u32> = (1..=page_count.min(4)).collect();
    if page_count > 4 {
        target_ords.push(page_count);
    }

    for page_ord in target_ords {
        let Ok(page) = doc.pages().get((page_ord - 1) as u16) else {
            continue;
        };
        let width_pts = page.width().value;
        let height_pts = page.height().value;
        let printed_label = page
            .label()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        if page_ord <= 3 {
            collect_page_identifier_links(&page, &mut page1_link_uris);
        }

        let mut spans = Vec::new();
        let mut raw_text = String::new();
        if let Ok(text_page) = page.text() {
            raw_text = text_page.all();
            if page_ord <= 3 {
                let mut char_boxes = Vec::new();
                for ch in text_page.chars().iter() {
                    if let Some(unicode) = ch.unicode_char() {
                        if let Ok(bounds) = ch.loose_bounds() {
                            char_boxes.push(PdfCharBox {
                                ch: unicode,
                                left: bounds.left().value,
                                bottom: bounds.bottom().value,
                                right: bounds.right().value,
                                top: bounds.top().value,
                            });
                        }
                    }
                }
                spans = group_pdf_chars_into_lines(&char_boxes, width_pts, height_pts);
            }
        }

        if !raw_text.trim().is_empty() {
            has_text_layer = true;
        }

        pages.push(ExtractedPage {
            page_ord,
            printed_label,
            width_pts,
            height_pts,
            spans,
            raw_text,
        });
    }

    let (doc_title, doc_author, mut extra_meta) = extract_pdf_metadata_fields(&doc);
    if !page1_link_uris.is_empty() {
        let joined_links = page1_link_uris.join(" \n ");
        extra_meta = Some(match extra_meta {
            Some(prev) => format!("{prev} \n {joined_links}"),
            None => joined_links,
        });
    }
    let xmp_xml = merge_xmp_and_extra_meta(extract_xmp_from_raw_pdf(path), extra_meta);

    Ok(ExtractedDocument {
        page_count,
        has_text_layer,
        pages,
        outline: Vec::new(),
        xmp_xml,
        doc_title,
        doc_author,
    })
}

fn collect_page_identifier_links(page: &PdfPage<'_>, out: &mut Vec<String>) {
    for link in page.links().iter() {
        if let Some(PdfAction::Uri(uri_action)) = link.action() {
            if let Ok(uri) = uri_action.uri() {
                let u = uri.trim();
                let low = u.to_ascii_lowercase();
                if low.contains("doi.org/10.")
                    || low.starts_with("10.")
                    || low.contains("arxiv.org/")
                    || low.contains("jstor.org/stable/")
                {
                    out.push(u.to_string());
                }
            }
        }
    }
}

fn extract_pdf_metadata_fields(
    doc: &PdfDocument<'_>,
) -> (Option<String>, Option<String>, Option<String>) {
    let metadata = doc.metadata();
    let doc_title = metadata
        .get(PdfDocumentMetadataTagType::Title)
        .map(|t| t.value().to_string())
        .filter(|s| !s.trim().is_empty());
    let doc_author = metadata
        .get(PdfDocumentMetadataTagType::Author)
        .map(|t| t.value().to_string())
        .filter(|s| !s.trim().is_empty());
    let subject = metadata
        .get(PdfDocumentMetadataTagType::Subject)
        .map(|t| t.value().to_string())
        .filter(|s| !s.trim().is_empty());
    let keywords = metadata
        .get(PdfDocumentMetadataTagType::Keywords)
        .map(|t| t.value().to_string())
        .filter(|s| !s.trim().is_empty());

    let mut extra_parts = Vec::new();
    if let Some(s) = subject {
        extra_parts.push(s);
    }
    if let Some(k) = keywords {
        extra_parts.push(k);
    }
    let extra = if extra_parts.is_empty() {
        None
    } else {
        Some(extra_parts.join(" \n "))
    };
    (doc_title, doc_author, extra)
}

fn merge_xmp_and_extra_meta(xmp: Option<String>, extra: Option<String>) -> Option<String> {
    match (xmp, extra) {
        (Some(x), Some(e)) => Some(format!("{x}\n<!-- pdf-meta: {e} -->")),
        (Some(x), None) => Some(x),
        (None, Some(e)) => Some(format!("<!-- pdf-meta: {e} -->")),
        (None, None) => None,
    }
}

fn classify_pdfium_load_error(path: &Path, err: PdfiumError) -> CoreError {
    let msg = format!("{err:?}");
    let lower = msg.to_lowercase();
    if lower.contains("password") || lower.contains("security") || lower.contains("encrypt") {
        CoreError::EncryptedFile(format!("{}: {msg}", path.display()))
    } else {
        CoreError::CorruptFile(format!("{}: {msg}", path.display()))
    }
}

fn extract_bookmarks(doc: &PdfDocument<'_>) -> Vec<OutlineItem> {
    let bookmarks = doc.bookmarks();
    collect_bookmark_siblings(bookmarks.root())
}

fn collect_bookmark_siblings(mut current: Option<PdfBookmark<'_>>) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    while let Some(bm) = current {
        let title = bm.title().unwrap_or_default();
        let page_ord = bm
            .destination()
            .and_then(|d| d.page_index().ok())
            .map(|idx| (idx as u32) + 1);
        let children = collect_bookmark_siblings(bm.first_child());
        if !title.trim().is_empty() || !children.is_empty() {
            items.push(OutlineItem {
                title,
                page_ord,
                children,
            });
        }
        current = bm.next_sibling();
    }
    items
}

/// Groups PDF characters on the same visual baseline into compact line `ExtractedSpan`s
/// with TOP-LEFT normalized coordinates in `[0.0, 1.0]`.
pub fn group_pdf_chars_into_lines(
    chars: &[PdfCharBox],
    page_w: f32,
    page_h: f32,
) -> Vec<ExtractedSpan> {
    if chars.is_empty() || page_w <= 0.0 || page_h <= 0.0 {
        return Vec::new();
    }

    let mut spans = Vec::new();
    let mut line_text = String::new();
    let mut line_bbox: Option<NormalizedRect> = None;
    let mut last_baseline_y: Option<f32> = None;
    let mut last_char_h: f32 = 10.0;
    let mut last_right: f32 = 0.0;

    let flush = |text: &mut String, bbox: &mut Option<NormalizedRect>, out: &mut Vec<ExtractedSpan>| {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            if let Some(b) = bbox.take() {
                out.push(ExtractedSpan {
                    text: trimmed.to_string(),
                    bbox: b,
                });
            }
        }
        text.clear();
        *bbox = None;
    };

    for c in chars {
        if c.ch == '\r' || c.ch == '\n' {
            flush(&mut line_text, &mut line_bbox, &mut spans);
            last_baseline_y = None;
            continue;
        }

        let char_h = (c.top - c.bottom).abs().max(2.0);
        let y_tol = (last_char_h.max(char_h) * 0.55).max(3.0);

        if let Some(prev_y) = last_baseline_y {
            let same_line = (c.bottom - prev_y).abs() <= y_tol;
            if !same_line {
                flush(&mut line_text, &mut line_bbox, &mut spans);
            } else if !line_text.is_empty()
                && !line_text.ends_with(' ')
                && !c.ch.is_whitespace()
                && (c.left - last_right) > char_h * 0.28
            {
                line_text.push(' ');
            }
        }

        if c.ch == '\0' {
            continue;
        }
        line_text.push(c.ch);
        last_baseline_y = Some(c.bottom);
        last_char_h = char_h;
        last_right = c.right;

        if let Some(rect) =
            NormalizedRect::from_bottom_left(c.left, c.bottom, c.right, c.top, page_w, page_h)
        {
            line_bbox = Some(match line_bbox {
                Some(existing) => existing.union(&rect),
                None => rect,
            });
        }
    }

    flush(&mut line_text, &mut line_bbox, &mut spans);
    spans
}

/// Scans the PDF head and tail cheaply for an embedded `<x:xmpmeta ... </x:xmpmeta>` packet.
fn extract_xmp_from_raw_pdf(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let total_len = file.metadata().ok()?.len();
    let chunk_cap = 512 * 1024usize;

    let mut buf = Vec::with_capacity(chunk_cap.min(total_len as usize));
    (&mut file)
        .take(chunk_cap as u64)
        .read_to_end(&mut buf)
        .ok()?;
    if let Some(xmp) = find_xmp_packet(&buf) {
        return Some(xmp);
    }

    if total_len > chunk_cap as u64 {
        let tail_start = total_len.saturating_sub(chunk_cap as u64);
        if file.seek(SeekFrom::Start(tail_start)).is_ok() {
            buf.clear();
            if file.read_to_end(&mut buf).is_ok() {
                if let Some(xmp) = find_xmp_packet(&buf) {
                    return Some(xmp);
                }
            }
        }
    }
    None
}

fn find_xmp_packet(buf: &[u8]) -> Option<String> {
    let lossy = String::from_utf8_lossy(buf);
    let start = lossy.find("<x:xmpmeta")?;
    let end = lossy[start..].find("</x:xmpmeta>")?;
    Some(lossy[start..start + end + "</x:xmpmeta>".len()].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_pdf_chars_into_top_left_line_spans() {
        // Page 600 x 800. Line 1 near top (bottom=700, top=712); Line 2 below (bottom=680, top=692)
        let chars = vec![
            PdfCharBox { ch: 'A', left: 60.0, bottom: 700.0, right: 70.0, top: 712.0 },
            PdfCharBox { ch: 'B', left: 70.0, bottom: 700.0, right: 80.0, top: 712.0 },
            PdfCharBox { ch: 'C', left: 90.0, bottom: 700.0, right: 100.0, top: 712.0 }, // space gap
            PdfCharBox { ch: 'D', left: 60.0, bottom: 680.0, right: 72.0, top: 692.0 },
        ];
        let spans = group_pdf_chars_into_lines(&chars, 600.0, 800.0);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "AB C");
        assert_eq!(spans[1].text, "D");
        // Line 1 is higher on page (smaller top-left y) than Line 2
        assert!(spans[0].bbox.y < spans[1].bbox.y);
        // Line 1 top_y = (800 - 712)/800 = 0.11
        assert!((spans[0].bbox.y - 0.11).abs() < 1e-5);
        assert!((spans[0].bbox.x - 0.10).abs() < 1e-5);
    }
}
