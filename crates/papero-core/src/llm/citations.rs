use crate::llm::guard::SelectedPassage;
use crate::llm::provider::LlmCitation;
use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

fn citation_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[Doc\s*#?(\d+)\s*,\s*p{1,2}\.?\s*(\d+)[^\]]*\]")
            .expect("valid citation regex")
    })
}

fn single_doc_page_citation_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[p{1,2}\.?\s*(\d+)[^\]]*\]").expect("valid single-doc citation regex")
    })
}

/// Extracts clickable `[Doc #id, p. X]` citations (and `[p. X]` citations when all passages
/// belong to a single document) from `answer` and resolves each claim back to the originating
/// `(file_id, page_ord, bboxes)` slice from `passages`.
pub fn parse_citations(answer: &str, passages: &[SelectedPassage]) -> Vec<LlmCitation> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for cap in citation_regex().captures_iter(answer) {
        let Some(doc_str) = cap.get(1).map(|m| m.as_str()) else {
            continue;
        };
        let Some(page_str) = cap.get(2).map(|m| m.as_str()) else {
            continue;
        };
        let Ok(doc_id) = doc_str.parse::<i64>() else {
            continue;
        };
        let Ok(page_ord) = page_str.parse::<u32>() else {
            continue;
        };
        let marker = cap.get(0).map(|m| m.as_str()).unwrap_or("").to_string();

        if !seen.insert((doc_id, page_ord)) {
            continue;
        }

        if let Some(exact) = passages
            .iter()
            .find(|p| p.document_id == doc_id && p.page_ord == page_ord)
        {
            out.push(LlmCitation {
                marker,
                document_id: exact.document_id,
                file_id: exact.file_id,
                page_ord: exact.page_ord,
                printed_label: exact.printed_label.clone(),
                doc_title: exact.doc_title.clone(),
                bboxes: exact.bboxes.clone(),
                snippet: make_preview(&exact.text_content),
            });
        } else if let Some(same_doc) = passages.iter().find(|p| p.document_id == doc_id) {
            out.push(LlmCitation {
                marker,
                document_id: same_doc.document_id,
                file_id: same_doc.file_id,
                page_ord,
                printed_label: None,
                doc_title: same_doc.doc_title.clone(),
                bboxes: Vec::new(),
                snippet: make_preview(&same_doc.text_content),
            });
        }
    }

    // When all passages come from a single document (e.g. "Ask this document" in viewer),
    // also resolve compact `[p. X]` markers directly to that document & page.
    let single_doc = passages.first().filter(|first| {
        passages
            .iter()
            .all(|p| p.document_id == first.document_id && p.file_id == first.file_id)
    });
    if let Some(base) = single_doc {
        for cap in single_doc_page_citation_regex().captures_iter(answer) {
            let Some(page_str) = cap.get(1).map(|m| m.as_str()) else {
                continue;
            };
            let Ok(page_ord) = page_str.parse::<u32>() else {
                continue;
            };
            if page_ord == 0 || !seen.insert((base.document_id, page_ord)) {
                continue;
            }
            let marker = cap.get(0).map(|m| m.as_str()).unwrap_or("").to_string();
            if let Some(exact) = passages.iter().find(|p| p.page_ord == page_ord) {
                out.push(LlmCitation {
                    marker,
                    document_id: exact.document_id,
                    file_id: exact.file_id,
                    page_ord: exact.page_ord,
                    printed_label: exact.printed_label.clone(),
                    doc_title: exact.doc_title.clone(),
                    bboxes: exact.bboxes.clone(),
                    snippet: make_preview(&exact.text_content),
                });
            } else {
                out.push(LlmCitation {
                    marker,
                    document_id: base.document_id,
                    file_id: base.file_id,
                    page_ord,
                    printed_label: None,
                    doc_title: base.doc_title.clone(),
                    bboxes: Vec::new(),
                    snippet: make_preview(&base.text_content),
                });
            }
        }
    }

    out
}

fn make_preview(text: &str) -> String {
    let trimmed = text.trim();
    let mut iter = trimmed.chars();
    let head: String = iter.by_ref().take(160).collect();
    if iter.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::types::NormalizedRect;

    #[test]
    fn resolves_citations_back_to_exact_file_page_and_bboxes() {
        let passages = vec![
            SelectedPassage {
                passage_id: 1,
                document_id: 42,
                file_id: 108,
                doc_title: "Functional Analysis".to_string(),
                page_ord: 17,
                printed_label: Some("11".to_string()),
                score: 4.5,
                text_content: "By the Hahn-Banach theorem, every bounded linear functional extends."
                    .to_string(),
                bboxes: vec![NormalizedRect {
                    x: 0.1,
                    y: 0.2,
                    w: 0.7,
                    h: 0.04,
                }],
            },
            SelectedPassage {
                passage_id: 2,
                document_id: 99,
                file_id: 205,
                doc_title: "Spectral Theory".to_string(),
                page_ord: 84,
                printed_label: None,
                score: 3.1,
                text_content: "A compact self-adjoint operator admits a countable orthonormal eigenbasis."
                    .to_string(),
                bboxes: vec![NormalizedRect {
                    x: 0.15,
                    y: 0.55,
                    w: 0.65,
                    h: 0.03,
                }],
            },
        ];

        let answer = "Hahn-Banach extends bounded functionals [Doc #42, p. 17] and compact self-adjoint operators diagonalize [Doc #99, p. 84] (repeated [Doc #42, p. 17]).";
        let cites = parse_citations(answer, &passages);

        assert_eq!(cites.len(), 2);
        assert_eq!(cites[0].document_id, 42);
        assert_eq!(cites[0].file_id, 108);
        assert_eq!(cites[0].page_ord, 17);
        assert_eq!(cites[0].bboxes.len(), 1);
        assert!((cites[0].bboxes[0].y - 0.2).abs() < 1e-6);

        assert_eq!(cites[1].document_id, 99);
        assert_eq!(cites[1].file_id, 205);
        assert_eq!(cites[1].page_ord, 84);
        assert_eq!(cites[1].bboxes.len(), 1);
    }
}
