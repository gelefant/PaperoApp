use crate::text_norm::normalize_math_text;

pub const MINHASH_LEN: usize = 128;

const MAX_MINHASH_WORDS: usize = 12_000;

/// Computes a 128-hash MinHash signature over normalized 5-word shingles.
/// Returns `None` when the text contains too few tokens (< 20 words) to avoid spurious collisions.
pub fn compute_minhash(text: &str) -> Option<[u64; MINHASH_LEN]> {
    let norm = normalize_math_text(text);
    let words: Vec<&str> = norm
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(MAX_MINHASH_WORDS)
        .collect();
    if words.len() < 20 {
        return None;
    }

    Some(compute_minhash_from_tokens(&words))
}

/// Computes a document-level MinHash signature by sampling substantive **interior body pages**
/// and stripping publisher/series/TOC boilerplate.
///
/// Why this matters for book series (*collane*):
/// Books in the same series often share 10–25 initial front-matter pages (series banner,
/// editorial board, series presentation, multi-page catalog of volumes in the series, copyright)
/// as well as trailing advertisement pages, or are scanned PDFs where only a digital series
/// cover sheet has selectable text. Sampling interior body pages ensures different books in the
/// same series never collide on front-matter while genuine copies/scans of the same book still
/// overlap strongly across their body chapters.
pub fn compute_document_minhash_from_pages<'a, I>(
    pages: I,
    total_page_count: u32,
) -> Option<[u64; MINHASH_LEN]>
where
    I: IntoIterator<Item = (u32, &'a str)>,
{
    // When `total_page_count` is already known, pre-bound the page range BEFORE coalescing
    // so 600-page books never allocate strings for front-matter or trailing chapters!
    let pre_bounds = if total_page_count >= 45 {
        let s = ((total_page_count * 18) / 100).clamp(14, 35);
        let e = ((total_page_count * 90) / 100).min(s + 90).max(s + 1);
        Some((s, e))
    } else if total_page_count >= 18 {
        let s = (total_page_count / 5).max(4);
        let e = total_page_count.saturating_sub(2).max(s);
        Some((s, e))
    } else {
        None
    };

    let (coalesced, max_seen_ord) = coalesce_pages(pages, pre_bounds);
    if coalesced.is_empty() {
        return None;
    }

    let effective_pages = total_page_count.max(max_seen_ord);

    let (start_ord, end_ord, min_words_per_page, min_body_pages, min_total_prose_words, max_sample_pages) =
        if effective_pages >= 45 {
            let start = ((effective_pages * 18) / 100).clamp(14, 35);
            let end = ((effective_pages * 90) / 100).max(start + 1);
            (start, end, 18usize, 4usize, 140usize, 40usize)
        } else if effective_pages >= 18 {
            let start = (effective_pages / 5).max(4);
            let end = effective_pages.saturating_sub(2).max(start);
            (start, end, 15usize, 2usize, 100usize, 30usize)
        } else {
            let min_words = if effective_pages <= 2 { 35usize } else { 70usize };
            (1u32, effective_pages.max(1), 8usize, 1usize, min_words, 18usize)
        };

    let mut sampled_pages = 0usize;
    let mut total_prose_words = 0usize;
    let mut body_buf = String::with_capacity(8192);

    for (ord, raw_page) in &coalesced {
        if *ord < start_ord || *ord > end_ord {
            continue;
        }
        if (2..18).contains(&effective_pages) && *ord == 1 && is_front_boilerplate_page(raw_page) {
            continue;
        }
        let Some((cleaned, prose_words)) = clean_body_page_text(raw_page) else {
            continue;
        };
        if prose_words < min_words_per_page {
            continue;
        }
        if !body_buf.is_empty() {
            body_buf.push('\n');
        }
        body_buf.push_str(&cleaned);
        total_prose_words += prose_words;
        sampled_pages += 1;

        if sampled_pages >= max_sample_pages || total_prose_words >= MAX_MINHASH_WORDS {
            break;
        }
    }

    if sampled_pages < min_body_pages || total_prose_words < min_total_prose_words {
        return None;
    }

    compute_minhash(&body_buf)
}

fn compute_minhash_from_tokens(words: &[&str]) -> [u64; MINHASH_LEN] {
    let mut sig = [u64::MAX; MINHASH_LEN];
    let window_size = words.len().min(5);
    for win in words.windows(window_size) {
        let base_h = fnv1a_shingle(win);
        for (i, slot) in sig.iter_mut().enumerate() {
            let h = mix_permutation(base_h, i as u64);
            if h < *slot {
                *slot = h;
            }
        }
    }
    sig
}

fn coalesce_pages<'a, I>(pages: I, bounds: Option<(u32, u32)>) -> (Vec<(u32, String)>, u32)
where
    I: IntoIterator<Item = (u32, &'a str)>,
{
    let mut out: Vec<(u32, String)> = Vec::new();
    let mut max_seen_ord = 0u32;
    let mut sorted = true;

    for (ord, txt) in pages {
        if ord == 0 || txt.trim().is_empty() {
            continue;
        }
        if ord > max_seen_ord {
            max_seen_ord = ord;
        }
        if let Some((lo, hi)) = bounds {
            if ord < lo || ord > hi {
                continue;
            }
        }
        if let Some((last_ord, last_txt)) = out.last_mut() {
            if *last_ord == ord {
                last_txt.push('\n');
                last_txt.push_str(txt);
                continue;
            }
            if ord < *last_ord {
                sorted = false;
            }
        }
        out.push((ord, txt.to_string()));
    }

    if sorted {
        return (out, max_seen_ord);
    }

    out.sort_by_key(|(ord, _)| *ord);
    let mut merged: Vec<(u32, String)> = Vec::with_capacity(out.len());
    for (ord, txt) in out {
        if let Some((last_ord, last_txt)) = merged.last_mut() {
            if *last_ord == ord {
                last_txt.push('\n');
                last_txt.push_str(&txt);
                continue;
            }
        }
        merged.push((ord, txt));
    }
    (merged, max_seen_ord)
}

fn is_front_boilerplate_page(raw: &str) -> bool {
    let low = raw.to_lowercase();
    if low.contains("abstract") || low.contains("1. introduction") {
        return false;
    }
    let words = raw.split_whitespace().count();
    if words == 0 {
        return true;
    }
    if words <= 220
        && (low.contains("jstor")
            || low.contains("numdam")
            || low.contains("eudml")
            || low.contains("hal.science")
            || low.contains("hal.archives-ouvertes")
            || low.contains("editorial board")
            || low.contains("series editors")
            || low.contains("comitato scientifico")
            || low.contains("direttore della collana")
            || low.contains("collana diretta da")
            || low.contains("piano dell'opera")
            || low.contains("presentazione della collana")
            || low.contains("nella stessa collana")
            || low.contains("volumi pubblicati")
            || low.contains("titles in this series")
            || low.contains("all rights reserved")
            || low.contains("tutti i diritti riservati"))
    {
        return true;
    }
    false
}

fn looks_like_toc_or_series_catalog_page(raw: &str) -> bool {
    let non_empty: Vec<&str> = raw.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if non_empty.is_empty() {
        return true;
    }
    for ln in non_empty.iter().take(6) {
        let low = ln
            .to_lowercase()
            .replace(['’', '‘'], "'");
        let stripped = low
            .trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | ' ' | '-' | '–'))
            .trim();
        if matches!(
            stripped,
            "contents"
                | "table of contents"
                | "indice"
                | "indice generale"
                | "indice analitico"
                | "sommario"
                | "inhalt"
                | "inhaltsverzeichnis"
                | "table des matières"
                | "table des matieres"
                | "elenco dei volumi"
                | "volumi pubblicati"
                | "volumi usciti"
                | "nella stessa collana"
                | "piano dell'opera"
                | "presentazione della collana"
                | "titles in this series"
                | "other titles in this series"
                | "volumes in this series"
                | "published titles"
        ) {
            return true;
        }
    }

    if non_empty.len() >= 8 {
        let toc_like = non_empty
            .iter()
            .filter(|ln| {
                let has_leader_dots = ln.contains("...") || ln.contains(". . .");
                let ends_with_page_num = ln
                    .rsplit_once(char::is_whitespace)
                    .is_some_and(|(left, right)| {
                        left.chars().filter(|c| c.is_alphabetic()).count() >= 4
                            && !right.is_empty()
                            && right.len() <= 4
                            && right.chars().all(|c| c.is_ascii_digit())
                    });
                has_leader_dots || ends_with_page_num
            })
            .count();
        if toc_like * 100 >= non_empty.len() * 45 {
            return true;
        }
    }

    false
}

fn is_boilerplate_line(low: &str) -> bool {
    const MARKERS: &[&str] = &[
        "all rights reserved",
        "tutti i diritti riservati",
        "nessuna parte di questa pubblicazione",
        "no part of this publication",
        "vietata la riproduzione",
        "library of congress",
        "cataloging-in-publication",
        "british library cataloguing",
        "printed in ",
        "stampato in ",
        "finito di stampare",
        "isbn ",
        "isbn:",
        "isbn-",
        "issn ",
        "issn:",
        "downloaded from",
        "jstor.org",
        "numdam.org",
        "eudml.org",
        "editorial board",
        "advisory board",
        "series editors",
        "comitato scientifico",
        "direttore della collana",
        "collana diretta da",
        "direzione scientifica",
        "nella stessa collana",
        "volumi pubblicati",
        "piano dell'opera",
        "supplemento a",
        "edizione speciale per",
        "non vendibile separatamente",
    ];
    MARKERS.iter().any(|m| low.contains(m))
}

fn clean_body_page_text(raw: &str) -> Option<(String, usize)> {
    if looks_like_toc_or_series_catalog_page(raw) {
        return None;
    }

    let mut kept_lines: Vec<&str> = Vec::new();
    let mut prose_words = 0usize;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.chars().filter(|c| c.is_alphabetic()).count() < 3 {
            continue;
        }
        let low = trimmed.to_lowercase().replace(['’', '‘'], "'");
        if is_boilerplate_line(&low) {
            continue;
        }
        let line_words = trimmed
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().filter(|c| c.is_alphabetic()).count() >= 2)
            .count();
        if line_words == 0 {
            continue;
        }
        prose_words += line_words;
        kept_lines.push(trimmed);
    }

    if kept_lines.is_empty() {
        None
    } else {
        Some((kept_lines.join("\n"), prose_words))
    }
}

pub fn pack_minhash(sig: &[u64; MINHASH_LEN]) -> Vec<u8> {
    let mut out = Vec::with_capacity(MINHASH_LEN * 8);
    for v in sig {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn unpack_minhash(blob: &[u8]) -> Option<[u64; MINHASH_LEN]> {
    if blob.len() != MINHASH_LEN * 8 {
        return None;
    }
    let mut out = [0u64; MINHASH_LEN];
    let (chunks, _) = blob.as_chunks::<8>();
    for (i, chunk) in chunks.iter().enumerate() {
        out[i] = u64::from_le_bytes(*chunk);
    }
    Some(out)
}

pub fn jaccard_similarity(a: &[u64; MINHASH_LEN], b: &[u64; MINHASH_LEN]) -> f32 {
    let equal = a.iter().zip(b.iter()).filter(|(x, y)| x == y).count();
    equal as f32 / MINHASH_LEN as f32
}

fn fnv1a_shingle(words: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for (idx, w) in words.iter().enumerate() {
        if idx > 0 {
            h ^= 0xff;
            h = h.wrapping_mul(0x100000001b3);
        }
        for &b in w.as_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    }
    h
}

#[inline]
fn mix_permutation(x: u64, idx: u64) -> u64 {
    let k1 = idx.wrapping_mul(0x9e3779b97f4a7c15).wrapping_add(0xbf58476d1ce4e5b9);
    let k2 = idx.wrapping_mul(0x94d049bb133111eb).wrapping_add(0x27d4eb2f165667c5);
    let mut z = x.wrapping_mul(k1 | 1).wrapping_add(k2);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_and_near_identical_texts_score_high() {
        let base = "Let X be a compact Kahler manifold with positive holomorphic sectional curvature. \
                    We prove by a Bochner-type formula and the Schwarz lemma that every rational curve \
                    on X satisfies the stability condition under Ricci flow perturbation.";
        let edited = "Let X be a compact Kähler manifold with positive holomorphic sectional curvature. \
                      We prove by a Bochner-type formula and the Schwarz lemma that every rational curve \
                      on X satisfies the stability condition under Ricci flow.";
        let unrelated = "In modular representation theory of finite groups of Lie type, Lusztig characters \
                         govern the decomposition numbers of blocks of defect zero and cyclic defect.";

        let s1 = compute_minhash(base).expect("sig1");
        let s2 = compute_minhash(edited).expect("sig2");
        let s3 = compute_minhash(unrelated).expect("sig3");

        let sim_close = jaccard_similarity(&s1, &s2);
        let sim_far = jaccard_similarity(&s1, &s3);
        assert!(sim_close > 0.70, "expected close similarity, got {sim_close}");
        assert!(sim_far < 0.25, "expected low similarity, got {sim_far}");

        let round = unpack_minhash(&pack_minhash(&s1)).expect("roundtrip");
        assert_eq!(s1, round);
    }

    #[test]
    fn scanned_book_with_only_front_collana_wrapper_returns_none() {
        let cover_wrapper = "Graduate Texts in Mathematics. Editorial Board: S. Axler, K.A. Ribet. \
            Other titles in this series: 1. Takeuti/Zaring Introduction to Axiomatic Set Theory; \
            2. Oxtoby Measure and Category; 3. Schaefer Topological Vector Spaces; \
            4. Hilton/Stammbach A Course in Homological Algebra; 5. Mac Lane Categories for the Working Mathematician.";
        let pages = vec![
            (1u32, cover_wrapper),
            (2u32, cover_wrapper),
            (3u32, cover_wrapper),
            (4u32, "Copyright 2019 Springer Nature Switzerland AG. All rights reserved."),
        ];
        // 280-page scanned book with text only on pages 1..4 must NOT produce a front-matter MinHash!
        assert!(compute_document_minhash_from_pages(pages, 280).is_none());
    }

    #[test]
    fn body_minhash_matches_shifted_book_scan_and_rejects_sister_collana_volume() {
        let collana_front = "Collana Scientifica diretta dal Comitato Editoriale Internazionale. \
            Presentazione della collana: un percorso rigoroso attraverso l'analisi matematica moderna, \
            la fisica teorica e la geometria algebrica contemporanea per studenti e ricercatori.";

        let mut book_a: Vec<(u32, String)> = Vec::new();
        let mut book_a_shifted: Vec<(u32, String)> = Vec::new();
        let mut book_b_same_collana: Vec<(u32, String)> = Vec::new();

        // Extra 3 scan cover pages on `book_a_shifted`
        for p in 1..=3u32 {
            book_a_shifted.push((p, format!("Digital archive scan header sheet {p}")));
        }
        for p in 1..=200u32 {
            let page_a = if p <= 22 {
                collana_front.to_string()
            } else {
                format!(
                    "Theorem {}.{} on Sobolev embeddings and Calderon-Zygmund singular integral operators \
                     over Lipschitz domains establishes interior elliptic regularity for weak solutions \
                     of divergence-form partial differential equations at step {}.",
                    p / 12,
                    p % 12,
                    p
                )
            };
            let page_b = if p <= 22 {
                collana_front.to_string()
            } else {
                format!(
                    "Proposition {}.{} in Galois cohomology of local non-archimedean fields proves \
                     Tate duality and reciprocity isomorphism for Lubin-Tate formal group modules \
                     with ramification filtration index {}.",
                    p / 10,
                    p % 10,
                    p * 7
                )
            };
            book_a.push((p, page_a.clone()));
            book_a_shifted.push((p + 3, page_a));
            book_b_same_collana.push((p, page_b));
        }

        let sig_a = compute_document_minhash_from_pages(
            book_a.iter().map(|(o, t)| (*o, t.as_str())),
            200,
        )
        .expect("sig_a");
        let sig_a_shift = compute_document_minhash_from_pages(
            book_a_shifted.iter().map(|(o, t)| (*o, t.as_str())),
            203,
        )
        .expect("sig_a_shift");
        let sig_b = compute_document_minhash_from_pages(
            book_b_same_collana.iter().map(|(o, t)| (*o, t.as_str())),
            200,
        )
        .expect("sig_b");

        let sim_same_book = jaccard_similarity(&sig_a, &sig_a_shift);
        let sim_sister_collana = jaccard_similarity(&sig_a, &sig_b);
        assert!(
            sim_same_book >= 0.80,
            "expected shifted scan of same book to match >= 0.80, got {sim_same_book}"
        );
        assert!(
            sim_sister_collana < 0.10,
            "expected sister volume in same collana to score < 0.10, got {sim_sister_collana}"
        );
    }
}
