use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

fn is_ot1_spacing_accent(ch: char) -> bool {
    matches!(
        ch,
        '\u{00A8}' // ¨ diaeresis
        | '\u{00B4}' // ´ acute
        | '\u{00B8}' // ¸ cedilla
        | '\u{00AF}' // ¯ macron
        | '\u{02C6}' // ˆ circumflex
        | '\u{02C7}' // ˇ caron
        | '\u{02D8}' // ˘ breve
        | '\u{02D9}' // ˙ dot above
        | '\u{02DA}' // ˚ ring above
        | '\u{02DB}' // ˛ ogonek
        | '\u{02DC}' // ˜ small tilde
        | '\u{02DD}' // ˝ double acute
    )
}

/// Expands PDF typographic ligatures (`ﬁ -> fi`, `ﬂ -> fl`, `ﬀ -> ff`, `ﬃ -> ffi`, `ﬄ -> ffl`),
/// strips soft hyphens, and collapses standalone OT1 LaTeX spacing accents (`Poincar´e -> Poincare`,
/// `H¨ormander -> Hormander`, `Poincar ´ e -> Poincare`) while preserving original letter case.
pub fn clean_ot1_and_ligatures_preserve_case(input: &str) -> String {
    if input.is_ascii() {
        return input.to_string();
    }
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len() + 8);
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        match ch {
            '\u{00AD}' | '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' => {
                i += 1;
            }
            '\u{FB00}' => {
                out.push_str("ff");
                i += 1;
            }
            '\u{FB01}' => {
                out.push_str("fi");
                i += 1;
            }
            '\u{FB02}' => {
                out.push_str("fl");
                i += 1;
            }
            '\u{FB03}' => {
                out.push_str("ffi");
                i += 1;
            }
            '\u{FB04}' => {
                out.push_str("ffl");
                i += 1;
            }
            '\u{FB05}' | '\u{FB06}' => {
                out.push_str("st");
                i += 1;
            }
            c if is_ot1_spacing_accent(c) => {
                // If Pdfium emitted "Poincar ´ e", drop the space right before and right after the accent
                let prev_was_space_after_letter = out.ends_with(' ')
                    && out
                        .chars()
                        .rev()
                        .nth(1)
                        .is_some_and(|p| p.is_alphabetic());
                let next_non_space_idx = if chars.get(i + 1) == Some(&' ') {
                    i + 2
                } else {
                    i + 1
                };
                let next_is_letter = chars
                    .get(next_non_space_idx)
                    .is_some_and(|n| n.is_alphabetic());
                if prev_was_space_after_letter && next_is_letter {
                    out.pop();
                    i = next_non_space_idx;
                } else if next_is_letter {
                    i = next_non_space_idx;
                } else {
                    i += 1;
                }
            }
            _ => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

/// Normalizes mathematical and European text for accent-tolerant, transliteration-aware
/// metadata and BM25 full-text search:
/// 1. Expands PDF ligatures (`ﬁ -> fi`, `ﬀ -> ff`, ...) and collapses OT1 LaTeX spacing accents.
/// 2. Pre-maps characters not decomposed by Unicode NFD (`ł -> l`, `ø -> o`, `ß -> ss`,
///    `đ -> d`, `æ -> ae`, `œ -> oe`, `þ -> th`, `ð -> d`, typographic quotes/dashes).
/// 3. Applies Unicode NFD decomposition and strips combining diacritical marks
///    (`Erdős -> Erdos`, `Čech -> Cech`, `Poincaré -> Poincare`, `Hölder -> Holder`).
/// 4. Lowercases the result.
pub fn normalize_math_text(input: &str) -> String {
    // Fast path for >98% of tokens (pure ASCII without backticks): single allocation, no NFD/Vec<char>.
    if input.bytes().all(|b| b < 0x80 && b != b'`') {
        return input.to_ascii_lowercase();
    }
    let expanded = clean_ot1_and_ligatures_preserve_case(input);
    let mut pre = String::with_capacity(expanded.len());
    for ch in expanded.chars() {
        match ch {
            'ł' | 'Ł' => pre.push('l'),
            'ø' | 'Ø' => pre.push('o'),
            'ß' | 'ẞ' => pre.push_str("ss"),
            'đ' | 'Đ' => pre.push('d'),
            'æ' | 'Æ' => pre.push_str("ae"),
            'œ' | 'Œ' => pre.push_str("oe"),
            'þ' | 'Þ' => pre.push_str("th"),
            'ð' | 'Ð' => pre.push('d'),
            '’' | '‘' | 'ʼ' | '`' => pre.push('\''),
            '–' | '—' | '−' | '\u{2010}' | '\u{2011}' | '\u{2012}' => pre.push('-'),
            _ => pre.push(ch),
        }
    }

    let mut out = String::with_capacity(pre.len());
    for ch in pre.nfd() {
        if !is_combining_mark(ch) {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_mathematician_names_and_diacritics() {
        assert_eq!(normalize_math_text("Paul Erdős"), "paul erdos");
        assert_eq!(normalize_math_text("Eduard Čech"), "eduard cech");
        assert_eq!(normalize_math_text("Henri Poincaré"), "henri poincare");
        assert_eq!(normalize_math_text("Kurt Gödel"), "kurt godel");
        assert_eq!(normalize_math_text("Otto Hölder"), "otto holder");
        assert_eq!(normalize_math_text("Étienne Bézout"), "etienne bezout");
        assert_eq!(normalize_math_text("Jan Łukasiewicz"), "jan lukasiewicz");
        assert_eq!(normalize_math_text("Øystein Ore"), "oystein ore");
        assert_eq!(normalize_math_text("Große"), "grosse");
        assert_eq!(normalize_math_text("Đuro Kurepa"), "duro kurepa");
        assert_eq!(normalize_math_text("Møbius–Bézout"), "mobius-bezout");
        // OT1 LaTeX PDF ligatures & standalone spacing accents
        assert_eq!(
            normalize_math_text("Mixed ﬁnite elements for Schr¨odinger diﬀusion"),
            "mixed finite elements for schrodinger diffusion"
        );
        assert_eq!(
            normalize_math_text("J.-C. N ´ ed ´ elec and I. Babusˇka"),
            "j.-c. nedelec and i. babuska"
        );
    }
}
