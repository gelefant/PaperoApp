use crate::extract::types::{pack_bboxes, ExtractedDocument, ExtractedPage, NormalizedRect};

/// A passage chunk ready for SQLite + Tantivy insertion.
#[derive(Debug, Clone)]
pub struct BuiltChunk {
    pub page_ord: u32,
    pub printed_label: Option<String>,
    pub chunk_index: u32,
    pub token_count: usize,
    pub text_content: String,
    pub bboxes_blob: Vec<u8>,
}

/// Estimates token count cleanly without loading an external ML tokenizer during Phase 1.
/// Whitespace words + CJK/math symbol segmentation roughly approximates subword tokens
/// (typically ~1.25 subwords per whitespace word in mathematical prose).
pub fn estimate_token_count(text: &str) -> usize {
    let mut words = 0usize;
    for w in text.split_whitespace() {
        let extra = w
            .chars()
            .filter(|c| !c.is_alphanumeric())
            .count()
            .min(2);
        words += 1 + usize::from(extra > 0);
    }
    words
}

/// Builds 300-500 token overlapping passage chunks per page while preserving line bounding boxes.
pub fn chunk_document(
    doc: &ExtractedDocument,
    target_tokens: usize,
    overlap_tokens: usize,
) -> Vec<BuiltChunk> {
    let target = target_tokens.clamp(200, 600);
    let overlap = overlap_tokens.min(target / 2);

    let mut all_chunks = Vec::new();
    for page in &doc.pages {
        let page_chunks = chunk_page(page, target, overlap);
        all_chunks.extend(page_chunks);
    }
    all_chunks
}

fn chunk_page(page: &ExtractedPage, target: usize, overlap: usize) -> Vec<BuiltChunk> {
    if page.spans.is_empty() {
        let trimmed = page.raw_text.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        return chunk_plain_text(page.page_ord, page.printed_label.as_deref(), trimmed, target, overlap);
    }

    let span_tokens: Vec<usize> = page
        .spans
        .iter()
        .map(|s| estimate_token_count(&s.text).max(1))
        .collect();

    let mut out = Vec::new();
    let mut start = 0usize;
    let mut chunk_index = 0u32;

    while start < page.spans.len() {
        let mut end = start;
        let mut tok_sum = 0usize;
        while end < page.spans.len() {
            let next_t = span_tokens[end];
            if tok_sum > 0 && tok_sum + next_t > target {
                break;
            }
            tok_sum += next_t;
            end += 1;
        }
        if end == start {
            // Single very long span: take it anyway so we always make forward progress.
            tok_sum = span_tokens[start];
            end = start + 1;
        }

        let mut text = String::new();
        let mut rects: Vec<NormalizedRect> = Vec::with_capacity(end - start);
        for span in &page.spans[start..end] {
            let t = span.text.trim();
            if !t.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            rects.push(span.bbox);
        }

        if !text.trim().is_empty() {
            out.push(BuiltChunk {
                page_ord: page.page_ord,
                printed_label: page.printed_label.clone(),
                chunk_index,
                token_count: tok_sum,
                text_content: text,
                bboxes_blob: pack_bboxes(&rects),
            });
            chunk_index += 1;
        }

        if end >= page.spans.len() {
            break;
        }

        // Backtrack from `end` to keep ~`overlap` tokens in the next chunk.
        let mut overlap_acc = 0usize;
        let mut next_start = end;
        while next_start > start + 1 {
            let prev = next_start - 1;
            if overlap_acc + span_tokens[prev] > overlap {
                break;
            }
            overlap_acc += span_tokens[prev];
            next_start = prev;
        }
        if next_start == start {
            next_start = start + 1;
        }
        start = next_start;
    }

    out
}

fn chunk_plain_text(
    page_ord: u32,
    printed_label: Option<&str>,
    text: &str,
    target: usize,
    overlap: usize,
) -> Vec<BuiltChunk> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let step = target.saturating_sub(overlap).max(1);
    let mut out = Vec::new();
    let mut idx = 0usize;
    let mut chunk_index = 0u32;

    while idx < words.len() {
        let end = (idx + target).min(words.len());
        let joined = words[idx..end].join(" ");
        let count = end - idx;
        out.push(BuiltChunk {
            page_ord,
            printed_label: printed_label.map(str::to_string),
            chunk_index,
            token_count: count,
            text_content: joined,
            bboxes_blob: Vec::new(),
        });
        chunk_index += 1;
        if end == words.len() {
            break;
        }
        idx += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::types::ExtractedSpan;

    #[test]
    fn builds_overlapping_chunks_with_bboxes() {
        let mut spans = Vec::new();
        for i in 0..80 {
            spans.push(ExtractedSpan {
                text: format!("Theorem {i} states every compact manifold admits a harmonic metric."),
                bbox: NormalizedRect {
                    x: 0.1,
                    y: (i as f32) * 0.01,
                    w: 0.8,
                    h: 0.01,
                },
            });
        }
        let doc = ExtractedDocument {
            page_count: 1,
            has_text_layer: true,
            pages: vec![ExtractedPage {
                page_ord: 1,
                printed_label: Some("1".into()),
                width_pts: 600.0,
                height_pts: 800.0,
                spans,
                raw_text: String::new(),
            }],
            outline: Vec::new(),
            xmp_xml: None,
            doc_title: None,
            doc_author: None,
        };

        let chunks = chunk_document(&doc, 350, 60);
        assert!(chunks.len() >= 2, "expected multiple chunks, got {}", chunks.len());
        assert!(!chunks[0].bboxes_blob.is_empty());
        // Verify overlap: last line of chunk 0 appears in chunk 1
        let last_line = chunks[0].text_content.lines().last().unwrap();
        assert!(chunks[1].text_content.contains(last_line));
    }
}
