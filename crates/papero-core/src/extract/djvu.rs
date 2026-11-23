use crate::config::{configure_background_command, resolve_external_tool};
use crate::error::{CoreError, Result};
use crate::extract::types::{
    ExtractedDocument, ExtractedPage, ExtractedSpan, NormalizedRect, OutlineItem,
};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq)]
enum SExpr {
    Atom(String),
    Str(String),
    List(Vec<SExpr>),
}

/// Extracts pages, dimensions, normalized top-left line bounding boxes, and outline
/// from a DjVu file by invoking the external `djvused` CLI (never linking `libdjvulibre`).
pub fn extract_djvu(path: &Path, djvused_bin: Option<&str>) -> Result<ExtractedDocument> {
    let bin = resolve_external_tool(djvused_bin, "djvused");

    // Step 1: get page count & outline in one quick invocation.
    let meta_out = run_djvused(&bin, path, "n; print-outline\n")?;
    let mut lines = meta_out.lines();
    let page_count: u32 = lines
        .next()
        .and_then(|l| l.trim().parse::<u32>().ok())
        .ok_or_else(|| {
            CoreError::CorruptFile(format!(
                "djvused did not return a valid page count for {}",
                path.display()
            ))
        })?;

    let outline_str: String = lines.collect::<Vec<_>>().join("\n");
    let outline = parse_djvu_outline(&outline_str);

    if page_count == 0 {
        return Ok(ExtractedDocument {
            page_count: 0,
            has_text_layer: false,
            pages: Vec::new(),
            outline,
            xmp_xml: None,
            doc_title: None,
            doc_author: None,
        });
    }

    // Step 2: query size + print-txt for every page in a single process via stdin script.
    let mut script = String::with_capacity((page_count as usize) * 28);
    for p in 1..=page_count {
        use std::fmt::Write as _;
        let _ = writeln!(script, "select {p}");
        script.push_str("size\nprint-txt\n");
    }

    let full_out = run_djvused(&bin, path, &script)?;
    let mut doc = parse_djvused_pages_output(&full_out, page_count, outline)?;
    if let Ok(meta_str) = run_djvused(&bin, path, "print-meta\n") {
        let (mt, ma) = parse_djvu_meta(&meta_str);
        doc.doc_title = mt;
        doc.doc_author = ma;
    }
    Ok(doc)
}

/// Fast front-matter extractor for DjVu files: inspects embedded `print-meta` plus
/// pages 1..=4 and the last page without scanning every page of large books.
pub fn extract_djvu_front_matter(path: &Path, djvused_bin: Option<&str>) -> Result<ExtractedDocument> {
    let bin = resolve_external_tool(djvused_bin, "djvused");
    let (page_count, doc_title, doc_author) = match run_djvused(&bin, path, "n\nprint-meta\n") {
        Ok(combined) => {
            let mut lines = combined.lines();
            let count = lines
                .next()
                .and_then(|l| l.trim().parse::<u32>().ok())
                .ok_or_else(|| {
                    CoreError::CorruptFile(format!(
                        "djvused did not return a valid page count for {}",
                        path.display()
                    ))
                })?;
            let meta_tail: String = lines.collect::<Vec<_>>().join("\n");
            let (mt, ma) = parse_djvu_meta(&meta_tail);
            (count, mt, ma)
        }
        Err(_) => {
            let count_out = run_djvused(&bin, path, "n\n")?;
            let count = count_out
                .lines()
                .next()
                .and_then(|l| l.trim().parse::<u32>().ok())
                .ok_or_else(|| {
                    CoreError::CorruptFile(format!(
                        "djvused did not return a valid page count for {}",
                        path.display()
                    ))
                })?;
            let (mt, ma) = run_djvused(&bin, path, "print-meta\n")
                .map(|s| parse_djvu_meta(&s))
                .unwrap_or((None, None));
            (count, mt, ma)
        }
    };

    if page_count == 0 {
        return Ok(ExtractedDocument {
            page_count: 0,
            has_text_layer: false,
            pages: Vec::new(),
            outline: Vec::new(),
            xmp_xml: None,
            doc_title,
            doc_author,
        });
    }

    let mut target_ords: Vec<u32> = (1..=page_count.min(6)).collect();
    if page_count > 6 {
        target_ords.push(page_count);
    }

    let mut script = String::with_capacity(target_ords.len() * 28);
    for &p in &target_ords {
        use std::fmt::Write as _;
        let _ = writeln!(script, "select {p}");
        script.push_str("size\nprint-txt\n");
    }

    let out = run_djvused(&bin, path, &script)?;
    let mut parsed = parse_djvused_pages_output(&out, target_ords.len() as u32, Vec::new())?;
    for (page, &real_ord) in parsed.pages.iter_mut().zip(target_ords.iter()) {
        page.page_ord = real_ord;
    }
    parsed.page_count = page_count;
    parsed.doc_title = doc_title;
    parsed.doc_author = doc_author;
    Ok(parsed)
}

fn parse_djvu_meta(raw: &str) -> (Option<String>, Option<String>) {
    let mut title = None;
    let mut author = None;
    for line in raw.lines() {
        let trimmed = line.trim();
        let Some((key, rest)) = trimmed.split_once(char::is_whitespace) else {
            continue;
        };
        let k_low = key.trim().to_ascii_lowercase();
        let val = rest.trim().trim_matches('"').trim();
        if val.is_empty() {
            continue;
        }
        if k_low == "title" && title.is_none() {
            title = Some(val.to_string());
        } else if (k_low == "author" || k_low == "creator") && author.is_none() {
            author = Some(val.to_string());
        }
    }
    (title, author)
}

/// Extracts selectable word/line spans with normalized top-left coordinates for a single DjVu page.
pub fn extract_djvu_single_page_spans(
    path: &Path,
    page_ord: u32,
    djvused_bin: Option<&str>,
) -> Result<Vec<ExtractedSpan>> {
    let bin = resolve_external_tool(djvused_bin, "djvused");
    let script = format!("select {page_ord}\nsize\nprint-txt\n");
    let out = run_djvused(&bin, path, &script)?;
    let mut lines = out.lines();
    let size_line = lines.next().unwrap_or("");
    let (mut page_w, mut page_h) = parse_size_line(size_line).unwrap_or((1000.0, 1400.0));
    let rest: String = lines.collect::<Vec<_>>().join("\n");
    let mut spans = Vec::new();
    if let Some((expr, _)) = parse_one_sexpr(&rest, 0) {
        if let SExpr::List(items) = &expr {
            if matches!(items.first(), Some(SExpr::Atom(t)) if t == "page") && items.len() >= 5 {
                if let (Some(xmax), Some(ymax)) = (as_f32(&items[3]), as_f32(&items[4])) {
                    if xmax > 0.0 && ymax > 0.0 {
                        page_w = xmax;
                        page_h = ymax;
                    }
                }
            }
        }
        collect_granular_text_spans(&expr, page_w, page_h, &mut spans);
    }
    Ok(spans)
}

fn collect_granular_text_spans(
    expr: &SExpr,
    page_w: f32,
    page_h: f32,
    spans: &mut Vec<ExtractedSpan>,
) {
    let SExpr::List(items) = expr else {
        return;
    };
    let Some(SExpr::Atom(tag)) = items.first() else {
        return;
    };
    if (tag == "word" || tag == "char") && items.len() >= 6 {
        if let (Some(xmin), Some(ymin), Some(xmax), Some(ymax)) = (
            as_f32(&items[1]),
            as_f32(&items[2]),
            as_f32(&items[3]),
            as_f32(&items[4]),
        ) {
            let mut txt = String::new();
            collect_strings_in_children(&items[5..], &mut txt);
            let trimmed = txt.trim();
            if !trimmed.is_empty() {
                if let Some(bbox) =
                    NormalizedRect::from_bottom_left(xmin, ymin, xmax, ymax, page_w, page_h)
                {
                    spans.push(ExtractedSpan {
                        text: trimmed.to_string(),
                        bbox,
                    });
                }
            }
        }
        return;
    }
    if tag == "line" && items.len() >= 6 {
        let has_nested_words = items[5..].iter().any(|child| {
            matches!(child, SExpr::List(c) if matches!(c.first(), Some(SExpr::Atom(a)) if a == "word" || a == "char"))
        });
        if has_nested_words {
            for child in &items[5..] {
                collect_granular_text_spans(child, page_w, page_h, spans);
            }
        } else if let (Some(xmin), Some(ymin), Some(xmax), Some(ymax)) = (
            as_f32(&items[1]),
            as_f32(&items[2]),
            as_f32(&items[3]),
            as_f32(&items[4]),
        ) {
            let mut line_text = String::new();
            collect_strings_in_children(&items[5..], &mut line_text);
            let trimmed = line_text.trim();
            if !trimmed.is_empty() {
                if let Some(bbox) =
                    NormalizedRect::from_bottom_left(xmin, ymin, xmax, ymax, page_w, page_h)
                {
                    spans.push(ExtractedSpan {
                        text: trimmed.to_string(),
                        bbox,
                    });
                }
            }
        }
        return;
    }
    for child in items.iter().skip(1) {
        collect_granular_text_spans(child, page_w, page_h, spans);
    }
}

fn run_djvused(bin: &str, file: &Path, script: &str) -> Result<String> {
    let mut cmd = Command::new(bin);
    configure_background_command(&mut cmd);
    let mut child = cmd
        .arg("-u")
        .arg(file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CoreError::MissingExternalTool {
                    tool: bin.to_string(),
                    detail: format!("could not spawn '{bin}': {e}"),
                }
            } else {
                CoreError::Io(e)
            }
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(CoreError::CorruptFile(format!(
            "djvused exited with {}: {}",
            output.status, stderr
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parses concatenated `width=W height=H\n(page ...)` blocks emitted by `djvused`.
pub fn parse_djvused_pages_output(
    output: &str,
    expected_pages: u32,
    outline: Vec<OutlineItem>,
) -> Result<ExtractedDocument> {
    let mut pages = Vec::with_capacity(expected_pages as usize);
    let mut has_text_layer = false;

    let bytes = output.as_bytes();
    let mut pos = 0usize;
    let mut current_page = 1u32;

    while pos < bytes.len() && current_page <= expected_pages {
        // Skip whitespace
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }

        // Expect `width=W height=H` line
        let line_end = output[pos..]
            .find('\n')
            .map(|rel| pos + rel)
            .unwrap_or(bytes.len());
        let size_line = output[pos..line_end].trim();
        let (w_px, h_px) = parse_size_line(size_line).unwrap_or((1000.0, 1400.0));
        pos = if line_end < bytes.len() {
            line_end + 1
        } else {
            bytes.len()
        };

        // Parse the following `(page ...)` S-expression if present.
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }

        let mut spans = Vec::new();
        let mut raw_text = String::new();
        let mut page_w = w_px;
        let mut page_h = h_px;

        if pos < bytes.len() && bytes[pos] == b'(' {
            if let Some((expr, next_pos)) = parse_one_sexpr(output, pos) {
                pos = next_pos;
                extract_page_sexpr(&expr, &mut page_w, &mut page_h, &mut spans, &mut raw_text);
            }
        }

        if !raw_text.trim().is_empty() {
            has_text_layer = true;
        }

        pages.push(ExtractedPage {
            page_ord: current_page,
            printed_label: None,
            width_pts: page_w,
            height_pts: page_h,
            spans,
            raw_text,
        });
        current_page += 1;
    }

    Ok(ExtractedDocument {
        page_count: expected_pages,
        has_text_layer,
        pages,
        outline,
        xmp_xml: None,
        doc_title: None,
        doc_author: None,
    })
}

fn parse_size_line(line: &str) -> Option<(f32, f32)> {
    let mut w = None;
    let mut h = None;
    for part in line.split_whitespace() {
        if let Some(val) = part.strip_prefix("width=") {
            w = val.parse::<f32>().ok();
        } else if let Some(val) = part.strip_prefix("height=") {
            h = val.parse::<f32>().ok();
        }
    }
    match (w, h) {
        (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some((w, h)),
        _ => None,
    }
}

fn extract_page_sexpr(
    expr: &SExpr,
    page_w: &mut f32,
    page_h: &mut f32,
    spans: &mut Vec<ExtractedSpan>,
    raw_text: &mut String,
) {
    let SExpr::List(items) = expr else {
        return;
    };
    if items.is_empty() {
        return;
    }
    if let SExpr::Atom(tag) = &items[0] {
        if tag == "page" && items.len() >= 5 {
            if let (Some(xmax), Some(ymax)) = (as_f32(&items[3]), as_f32(&items[4])) {
                if xmax > 0.0 && ymax > 0.0 {
                    *page_w = xmax;
                    *page_h = ymax;
                }
            }
            for child in &items[5..] {
                collect_djvu_text_nodes(child, *page_w, *page_h, spans, raw_text);
            }
            return;
        }
    }
    collect_djvu_text_nodes(expr, *page_w, *page_h, spans, raw_text);
}

fn collect_djvu_text_nodes(
    expr: &SExpr,
    page_w: f32,
    page_h: f32,
    spans: &mut Vec<ExtractedSpan>,
    raw_text: &mut String,
) {
    let SExpr::List(items) = expr else {
        return;
    };
    let Some(SExpr::Atom(tag)) = items.first() else {
        return;
    };

    if tag == "line" && items.len() >= 6 {
        if let (Some(xmin), Some(ymin), Some(xmax), Some(ymax)) = (
            as_f32(&items[1]),
            as_f32(&items[2]),
            as_f32(&items[3]),
            as_f32(&items[4]),
        ) {
            let mut line_text = String::new();
            collect_strings_in_children(&items[5..], &mut line_text);
            let trimmed = line_text.trim();
            if !trimmed.is_empty() {
                if !raw_text.is_empty() {
                    raw_text.push('\n');
                }
                raw_text.push_str(trimmed);
                if let Some(bbox) =
                    NormalizedRect::from_bottom_left(xmin, ymin, xmax, ymax, page_w, page_h)
                {
                    spans.push(ExtractedSpan {
                        text: trimmed.to_string(),
                        bbox,
                    });
                }
            }
        }
        return;
    }

    if (tag == "word" || tag == "char") && items.len() >= 6 {
        if let (Some(xmin), Some(ymin), Some(xmax), Some(ymax)) = (
            as_f32(&items[1]),
            as_f32(&items[2]),
            as_f32(&items[3]),
            as_f32(&items[4]),
        ) {
            let mut txt = String::new();
            collect_strings_in_children(&items[5..], &mut txt);
            let trimmed = txt.trim();
            if !trimmed.is_empty() {
                if !raw_text.is_empty() {
                    raw_text.push(' ');
                }
                raw_text.push_str(trimmed);
                if let Some(bbox) =
                    NormalizedRect::from_bottom_left(xmin, ymin, xmax, ymax, page_w, page_h)
                {
                    spans.push(ExtractedSpan {
                        text: trimmed.to_string(),
                        bbox,
                    });
                }
            }
        }
        return;
    }

    // Container nodes: column, region, para, etc.
    for child in items.iter().skip(1) {
        collect_djvu_text_nodes(child, page_w, page_h, spans, raw_text);
    }
}

fn collect_strings_in_children(nodes: &[SExpr], out: &mut String) {
    for node in nodes {
        match node {
            SExpr::Str(s) => {
                let t = s.trim();
                if !t.is_empty() {
                    if !out.is_empty() {
                        out.push(' ');
                    }
                    out.push_str(t);
                }
            }
            SExpr::List(children) => {
                // Skip leading tag & 4 coord numbers if it's a nested (word ...) or (char ...)
                let start_idx = if matches!(children.first(), Some(SExpr::Atom(a)) if a == "word" || a == "char" || a == "line")
                {
                    5.min(children.len())
                } else {
                    0
                };
                collect_strings_in_children(&children[start_idx..], out);
            }
            SExpr::Atom(_) => {}
        }
    }
}

fn as_f32(expr: &SExpr) -> Option<f32> {
    match expr {
        SExpr::Atom(a) => a.parse::<f32>().ok(),
        _ => None,
    }
}

pub fn parse_djvu_outline(input: &str) -> Vec<OutlineItem> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Some((expr, _)) = parse_one_sexpr(trimmed, 0) else {
        return Vec::new();
    };
    let SExpr::List(items) = expr else {
        return Vec::new();
    };
    let start = if matches!(items.first(), Some(SExpr::Atom(a)) if a == "bookmarks") {
        1
    } else {
        0
    };
    items[start..]
        .iter()
        .filter_map(parse_bookmark_node)
        .collect()
}

fn parse_bookmark_node(expr: &SExpr) -> Option<OutlineItem> {
    let SExpr::List(items) = expr else {
        return None;
    };
    let title = match items.first()? {
        SExpr::Str(s) => s.clone(),
        _ => return None,
    };
    let mut page_ord = None;
    let mut child_start = 1usize;
    if let Some(SExpr::Str(target)) = items.get(1) {
        if let Some(num) = target.strip_prefix('#') {
            page_ord = num.parse::<u32>().ok();
        }
        child_start = 2;
    }
    let children = items[child_start..]
        .iter()
        .filter_map(parse_bookmark_node)
        .collect();
    Some(OutlineItem {
        title,
        page_ord,
        children,
    })
}

fn parse_one_sexpr(input: &str, start: usize) -> Option<(SExpr, usize)> {
    let bytes = input.as_bytes();
    let mut pos = start;
    while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
        pos += 1;
    }
    if pos >= bytes.len() {
        return None;
    }
    match bytes[pos] {
        b'(' => {
            pos += 1;
            let mut items = Vec::new();
            loop {
                while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                    pos += 1;
                }
                if pos >= bytes.len() {
                    break;
                }
                if bytes[pos] == b')' {
                    pos += 1;
                    break;
                }
                let (child, next) = parse_one_sexpr(input, pos)?;
                items.push(child);
                pos = next;
            }
            Some((SExpr::List(items), pos))
        }
        b'"' => {
            pos += 1;
            let mut buf: Vec<u8> = Vec::new();
            while pos < bytes.len() {
                let b = bytes[pos];
                if b == b'"' {
                    pos += 1;
                    break;
                }
                if b == b'\\' && pos + 1 < bytes.len() {
                    pos += 1;
                    let esc = bytes[pos];
                    match esc {
                        b'n' => {
                            buf.push(b'\n');
                            pos += 1;
                        }
                        b'r' => {
                            buf.push(b'\r');
                            pos += 1;
                        }
                        b't' => {
                            buf.push(b'\t');
                            pos += 1;
                        }
                        b'\\' => {
                            buf.push(b'\\');
                            pos += 1;
                        }
                        b'"' => {
                            buf.push(b'"');
                            pos += 1;
                        }
                        b'0'..=b'7' => {
                            let mut val = esc - b'0';
                            pos += 1;
                            for _ in 0..2 {
                                if pos < bytes.len() && (b'0'..=b'7').contains(&bytes[pos]) {
                                    val = val.wrapping_mul(8).wrapping_add(bytes[pos] - b'0');
                                    pos += 1;
                                } else {
                                    break;
                                }
                            }
                            buf.push(val);
                        }
                        other => {
                            buf.push(other);
                            pos += 1;
                        }
                    }
                } else {
                    buf.push(b);
                    pos += 1;
                }
            }
            let s = String::from_utf8_lossy(&buf).into_owned();
            Some((SExpr::Str(s), pos))
        }
        _ => {
            let begin = pos;
            while pos < bytes.len()
                && !bytes[pos].is_ascii_whitespace()
                && bytes[pos] != b'('
                && bytes[pos] != b')'
                && bytes[pos] != b'"'
            {
                pos += 1;
            }
            Some((SExpr::Atom(input[begin..pos].to_string()), pos))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_djvused_bottom_left_coords_and_octal_utf8() {
        let sample = concat!(
            "width=1000 height=2000\n",
            "(page 0 0 1000 2000\n",
            " (column\n",
            "  (para\n",
            "   (line 100 1600 900 1800 \"Theorem \\\"1\\\" by Erd\\305\\221s\")\n",
            "   (line 100 200 500 300 (word 100 200 250 300 \"Proof\") (word 270 200 500 300 \"complete.\")))))\n",
            "width=1000 height=2000\n",
            "(page 0 0 0 0 \"\")\n"
        );

        let doc = parse_djvused_pages_output(sample, 2, Vec::new()).unwrap();
        assert_eq!(doc.page_count, 2);
        assert!(doc.has_text_layer);
        assert_eq!(doc.pages.len(), 2);

        let p1 = &doc.pages[0];
        assert_eq!(p1.spans.len(), 2);
        assert_eq!(p1.spans[0].text, "Theorem \"1\" by Erdős");
        // ymin=1600, ymax=1800 on H=2000 -> top_y = (2000 - 1800)/2000 = 0.10
        assert!((p1.spans[0].bbox.x - 0.10).abs() < 1e-5);
        assert!((p1.spans[0].bbox.y - 0.10).abs() < 1e-5);
        assert!((p1.spans[0].bbox.w - 0.80).abs() < 1e-5);
        assert!((p1.spans[0].bbox.h - 0.10).abs() < 1e-5);

        assert_eq!(p1.spans[1].text, "Proof complete.");
        assert!(doc.pages[1].spans.is_empty());
    }

    #[test]
    fn parses_djvu_outline() {
        let outline_txt = r##"(bookmarks ("Chapter 1" "#1" ("Section 1.1" "#3")) ("Appendix" "#42"))"##;
        let items = parse_djvu_outline(outline_txt);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Chapter 1");
        assert_eq!(items[0].page_ord, Some(1));
        assert_eq!(items[0].children.len(), 1);
        assert_eq!(items[0].children[0].page_ord, Some(3));
        assert_eq!(items[1].page_ord, Some(42));
    }
}
