use crate::config::bundled_resource_dirs;
use crate::error::{CoreError, Result};
use crate::extract::types::{
    ExtractedDocument, ExtractedPage, ExtractedSpan, NormalizedRect, OutlineItem,
};
use pdfium_render::prelude::*;
use std::any::Any;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

const PDFIUM_WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;
const MAX_BOOKMARK_DEPTH: usize = 64;
const MAX_BOOKMARK_NODES: usize = 10_000;

type PdfiumTask = Box<dyn FnOnce(Result<&Pdfium>) + Send>;
type PdfiumWorkItem = (Option<PathBuf>, PdfiumTask);
type PdfiumSender = Mutex<Sender<PdfiumWorkItem>>;

static PDFIUM_WORKER_TX: OnceLock<std::result::Result<PdfiumSender, String>> = OnceLock::new();

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn pdfium_worker_sender() -> Result<&'static PdfiumSender> {
    let init = PDFIUM_WORKER_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<PdfiumWorkItem>();
        std::thread::Builder::new()
            .name("papero-pdfium".to_string())
            .stack_size(PDFIUM_WORKER_STACK_BYTES)
            .spawn(move || {
                let mut pdfium_opt: Option<Pdfium> = None;
                while let Ok((explicit_path, task)) = rx.recv() {
                    if pdfium_opt.is_none() {
                        match bind_pdfium(explicit_path.as_deref()) {
                            Ok(instance) => {
                                pdfium_opt = Some(instance);
                            }
                            Err(err) => {
                                task(Err(err));
                                continue;
                            }
                        }
                    }
                    let pdfium_ref = pdfium_opt.as_ref().expect("pdfium initialized on worker");
                    task(Ok(pdfium_ref));
                }
            })
            .map(|_| Mutex::new(tx))
            .map_err(|e| format!("failed spawning dedicated PDFium thread: {e}"))
    });
    match init {
        Ok(sender) => Ok(sender),
        Err(msg) => Err(CoreError::Extract(msg.clone())),
    }
}

/// Executes `f` on Papero's single long-lived, 16 MB-stack `papero-pdfium` worker thread so:
/// 1. `Pdfium` (`!Send`, `!Sync`) never crosses OS threads and `FPDF_InitLibrary()` stays bound
///    to an immortal thread (preventing Win32 `DLL_THREAD_DETACH` / GDI+ / TLS teardown crashes
///    when temporary Rayon worker pools exit),
/// 2. Deep C++ PDF object/font/form recursion inside `pdfium.dll` always has 16 MB of stack even
///    on Windows (where default thread stacks are 1 MB),
/// 3. Any Rust panic inside `f` is caught via `catch_unwind` and returned as `CoreError::Extract`.
pub fn with_pdfium<T, F>(explicit_path: Option<&Path>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Pdfium) -> Result<T> + Send + 'static,
{
    let sender_mutex = pdfium_worker_sender()?;
    let (res_tx, res_rx) = mpsc::sync_channel::<Result<T>>(1);
    let explicit_owned = explicit_path.map(Path::to_path_buf);

    let task: PdfiumTask = Box::new(move |pdfium_res| {
        let outcome = match pdfium_res {
            Ok(pdfium) => {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(pdfium))) {
                    Ok(r) => r,
                    Err(payload) => Err(CoreError::Extract(format!(
                        "PDFium worker caught panic: {}",
                        panic_payload_message(payload.as_ref())
                    ))),
                }
            }
            Err(err) => Err(err),
        };
        let _ = res_tx.send(outcome);
    });

    {
        let tx = sender_mutex.lock().unwrap_or_else(|e| e.into_inner());
        tx.send((explicit_owned, task))
            .map_err(|_| CoreError::Extract("PDFium worker thread terminated unexpectedly".into()))?;
    }

    res_rx
        .recv()
        .map_err(|_| CoreError::Extract("PDFium worker channel closed before completion".into()))?
}

/// Buffered exact-block `Read + Seek` adapter for `FPDF_LoadCustomDocument`.
///
/// `pdfium-render`'s `read_block_from_callback` invokes `reader.read(...)` once and returns
/// `bytes_read as c_int` directly to PDFium (`m_GetBlock`), which treats ANY non-zero value as
/// boolean `TRUE` ("full requested block is valid"). Looping until the entire slice is filled
/// (and returning `UnexpectedEof` -> `0` on short reads) prevents uninitialized-buffer reads on
/// Windows/cloud filesystems while 64 KB buffering eliminates thousands of tiny Win32 syscalls.
struct PdfiumExactReader<R> {
    inner: R,
}

impl<R: Read> Read for PdfiumExactReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut filled = 0usize;
        while filled < buf.len() {
            match self.inner.read(&mut buf[filled..]) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "short read while fulfilling PDFium FPDF_FILEACCESS block",
                    ));
                }
                Ok(n) => filled += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(filled)
    }
}

impl<R: Seek> Seek for PdfiumExactReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// Opens a `PdfDocument` safely using a 64 KB buffered exact-block reader and validates that
/// file length fits inside `FPDF_FILEACCESS::m_FileLen` (`c_ulong`, 32-bit `u32` on 64-bit Windows).
pub(crate) fn open_pdf_document<'a>(pdfium: &'a Pdfium, path: &Path) -> Result<PdfDocument<'a>> {
    let file = std::fs::File::open(path).map_err(|e| {
        CoreError::Io(std::io::Error::new(
            e.kind(),
            format!("failed opening PDF {}: {e}", path.display()),
        ))
    })?;
    let file_len = file
        .metadata()
        .map_err(|e| {
            CoreError::Io(std::io::Error::new(
                e.kind(),
                format!("failed reading PDF metadata {}: {e}", path.display()),
            ))
        })?
        .len();
    if file_len == 0 {
        return Err(CoreError::CorruptFile(format!(
            "{}: empty PDF file (0 bytes)",
            path.display()
        )));
    }
    #[allow(clippy::unnecessary_cast)]
    let max_file_access_len = std::os::raw::c_ulong::MAX as u64;
    if file_len > max_file_access_len {
        return Err(CoreError::CorruptFile(format!(
            "{}: PDF exceeds FPDF_FILEACCESS size limit ({file_len} bytes)",
            path.display()
        )));
    }
    let reader = PdfiumExactReader {
        inner: BufReader::with_capacity(64 * 1024, file),
    };
    pdfium
        .load_pdf_from_reader(reader, None)
        .map_err(|err| classify_pdfium_load_error(path, err))
}

#[cfg(target_os = "windows")]
fn preload_windows_crt_beside(lib_path: &Path) {
    let dir = if lib_path.is_dir() {
        Some(lib_path)
    } else {
        lib_path.parent()
    };
    if let Some(d) = dir {
        for crt_dll in [
            "vcruntime140.dll",
            "vcruntime140_1.dll",
            "msvcp140.dll",
            "msvcp140_1.dll",
        ] {
            let candidate = d.join(crt_dll);
            if candidate.is_file() {
                let _ = ort::util::preload_dylib(&candidate);
            }
        }
    }
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
        #[cfg(target_os = "windows")]
        preload_windows_crt_beside(p);
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
        #[cfg(target_os = "windows")]
        preload_windows_crt_beside(p);
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
            #[cfg(target_os = "windows")]
            preload_windows_crt_beside(&lib_candidate);
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
    let path_buf = path.to_path_buf();
    with_pdfium(pdfium_lib_path, move |pdfium| {
        extract_pdf_on_worker(pdfium, &path_buf)
    })
}

fn extract_pdf_on_worker(pdfium: &Pdfium, path: &Path) -> Result<ExtractedDocument> {
    let doc = open_pdf_document(pdfium, path)?;

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
/// (head + tail), pages 1..=3 with character/line bounding boxes (`spans`), pages 4..=6
/// text, and the final page text without parsing the entire body of large documents.
pub fn extract_pdf_front_matter(
    path: &Path,
    pdfium_lib_path: Option<&Path>,
) -> Result<ExtractedDocument> {
    let path_buf = path.to_path_buf();
    with_pdfium(pdfium_lib_path, move |pdfium| {
        extract_pdf_front_matter_on_worker(pdfium, &path_buf)
    })
}

fn extract_pdf_front_matter_on_worker(pdfium: &Pdfium, path: &Path) -> Result<ExtractedDocument> {
    let doc = open_pdf_document(pdfium, path)?;

    let page_count = doc.pages().len() as u32;
    let mut pages = Vec::new();
    let mut has_text_layer = false;
    let mut page1_link_uris: Vec<String> = Vec::new();

    let mut target_ords: Vec<u32> = (1..=page_count.min(6)).collect();
    if page_count > 6 {
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
    let mut seen_nodes = 0usize;
    collect_bookmark_siblings(bookmarks.root(), 0, &mut seen_nodes)
}

fn collect_bookmark_siblings(
    mut current: Option<PdfBookmark<'_>>,
    depth: usize,
    seen_nodes: &mut usize,
) -> Vec<OutlineItem> {
    if depth >= MAX_BOOKMARK_DEPTH || *seen_nodes >= MAX_BOOKMARK_NODES {
        return Vec::new();
    }
    let mut items = Vec::new();
    while let Some(bm) = current {
        if *seen_nodes >= MAX_BOOKMARK_NODES {
            break;
        }
        *seen_nodes += 1;
        let title = bm.title().unwrap_or_default();
        let page_ord = bm
            .destination()
            .and_then(|d| d.page_index().ok())
            .map(|idx| (idx as u32) + 1);
        let children = collect_bookmark_siblings(bm.first_child(), depth + 1, seen_nodes);
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

    struct ShortChunkReader<'a> {
        data: &'a [u8],
        pos: usize,
        max_chunk: usize,
    }

    impl Read for ShortChunkReader<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.data.len() || buf.is_empty() {
                return Ok(0);
            }
            let n = (self.data.len() - self.pos)
                .min(buf.len())
                .min(self.max_chunk);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn exact_reader_fills_full_block_across_short_reads_and_errors_on_truncation() {
        let payload = b"0123456789abcdef";
        let mut reader = PdfiumExactReader {
            inner: ShortChunkReader {
                data: payload,
                pos: 0,
                max_chunk: 3,
            },
        };
        let mut out = [0u8; 10];
        assert_eq!(reader.read(&mut out).unwrap(), 10);
        assert_eq!(&out, b"0123456789");

        // Requesting 8 bytes when only 6 remain must fail with UnexpectedEof instead of partial read.
        let mut tail = [0u8; 8];
        let err = reader.read(&mut tail).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn dedicated_pdfium_worker_survives_ephemeral_caller_thread_exit_and_extracts_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let pdf_path = dir.path().join("ephemeral_thread_test.pdf");
        let pdf_for_create = pdf_path.clone();

        // Step 1: Initialize and invoke PDFium from a short-lived thread that exits immediately
        // (simulating Rayon pool teardown after `run_extract_bm25_queue`).
        let created = std::thread::spawn(move || {
            with_pdfium(None, move |pdfium| {
                let mut doc = pdfium
                    .create_new_pdf()
                    .map_err(|e| CoreError::Extract(format!("create_new_pdf: {e:?}")))?;
                let font = doc.fonts_mut().helvetica();
                let mut page = doc
                    .pages_mut()
                    .create_page_at_end(PdfPagePaperSize::a4())
                    .map_err(|e| CoreError::Extract(format!("create_page: {e:?}")))?;
                let mut text_obj = PdfPageTextObject::new(
                    &doc,
                    "Theorem 4.2 (Hodge Decomposition on Compact Manifolds)",
                    font,
                    PdfPoints::new(14.0),
                )
                .map_err(|e| CoreError::Extract(format!("text_obj: {e:?}")))?;
                text_obj
                    .translate(PdfPoints::new(72.0), PdfPoints::new(720.0))
                    .map_err(|e| CoreError::Extract(format!("translate: {e:?}")))?;
                page.objects_mut()
                    .add_text_object(text_obj)
                    .map_err(|e| CoreError::Extract(format!("add_text: {e:?}")))?;
                doc.save_to_file(&pdf_for_create)
                    .map_err(|e| CoreError::Extract(format!("save_to_file: {e:?}")))?;
                Ok(())
            })
        })
        .join()
        .unwrap();

        if let Err(CoreError::MissingExternalTool { .. }) = created {
            // Bare CI runners without staged libpdfium skip cleanly.
            return;
        }
        created.expect("PDF creation on dedicated PDFium worker succeeded");

        // Step 2: Extract from multiple fresh small-stack threads after the creator thread died.
        let mut handles = Vec::new();
        for i in 0..4 {
            let p = pdf_path.clone();
            handles.push(
                std::thread::Builder::new()
                    .stack_size(512 * 1024)
                    .spawn(move || {
                        if i % 2 == 0 {
                            extract_pdf(&p, None).unwrap()
                        } else {
                            extract_pdf_front_matter(&p, None).unwrap()
                        }
                    })
                    .unwrap(),
            );
        }
        for h in handles {
            let extracted = h.join().unwrap();
            assert_eq!(extracted.page_count, 1);
            assert!(extracted.has_text_layer);
            assert!(extracted.pages[0]
                .raw_text
                .contains("Hodge Decomposition"));
        }
    }
}
