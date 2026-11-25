use regex::Regex;
use std::sync::OnceLock;

/// A detected mathematical statement header (Theorem, Lemma, Proposition, Corollary, Definition,
/// Conjecture, Axiom, Claim, Remark, Example) inside a passage chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedStatement {
    /// Canonical lowercase kind:
    /// `"theorem" | "lemma" | "proposition" | "corollary" | "definition" | "conjecture" | "axiom" | "claim" | "remark" | "example"`.
    pub kind: &'static str,
    /// Human-readable short badge, e.g. `"Theorem 3.2"` or `"Remark 2.1"`.
    pub label: String,
}

fn statement_header_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Matches real mathematical statement headers at the start of a line or sentence
        // (English, Italian, French, German) with optional numbering like `3.2`, `2.1.4`, `A.1`, `(a)`.
        // Requires either a number, a parenthetical attribution `(Riesz)`, or explicit punctuation (`.` / `:` / `—`)
        // to eliminate casual prose occurrences ("a theorem about...").
        Regex::new(
            r"(?im)(?:^|[.!?]\s+|\n\s*)(Theorem|Teorema|Th[eé]or[eè]me|Satz|Hauptsatz|Lemma|Lemme|Hilfssatz|Proposition|Proposizione|Corollary|Corollario|Corollaire|Korollar|Definition|Definizione|D[eé]finition|Conjecture|Congettura|Axiom|Assioma|Axiome|Claim|Remark|Remarks|Osservazione|Osservazioni|Remarque|Remarques|Bemerkung|Anmerkung|Example|Examples|Esempio|Esempi|Exemple|Exemples|Beispiel|Counterexample|Controesempio|Contre-exemple|Gegenbeispiel)(?:\s+([A-Z]?\d+(?:[.\-]\d+)*[a-z]?|[IVXLCM]+|\*[A-Z0-9]*))?(?:\s*\([^)\n]{1,48}\))?\s*(?:[.:—–-]|\n|$)",
        )
        .unwrap()
    })
}

fn prose_ref_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Rejects back-references like "By Theorem 3.2", "see Lemma 1", "in Proposition 2", "dal Teorema 1".
        Regex::new(
            r"(?i)\b(?:by|from|in|of|to|see|cf\.?|using|via|apply|applying|prove|proving|proved|proof\s+of|as\s+in|compare|recall|hence|then|since|with|under|per|dal|dalla|del|della|nel|nella|vedi|v\.|cfr\.?|da|di|par|d'apr[eè]s|selon|voir|dans|nach|aus|gem[aä][sß]|siehe|laut|wie\s+in)\s+(?:the|a|an|il|lo|la|le|un|una|der|die|das|dem|den)?\s*$",
        )
        .unwrap()
    })
}

fn canonical_kind(keyword: &str) -> &'static str {
    let low = keyword.trim().to_ascii_lowercase();
    if low.starts_with("theo")
        || low.starts_with("théo")
        || low.starts_with("teor")
        || low == "satz"
        || low == "hauptsatz"
    {
        "theorem"
    } else if low.starts_with("lem") || low == "hilfssatz" {
        "lemma"
    } else if low.starts_with("prop") {
        "proposition"
    } else if low.starts_with("cor") || low.starts_with("kor") {
        "corollary"
    } else if low.starts_with("def") || low.starts_with("déf") {
        "definition"
    } else if low.starts_with("conj") || low.starts_with("cong") {
        "conjecture"
    } else if low.starts_with("ax") || low.starts_with("ass") {
        "axiom"
    } else if low.starts_with("rem")
        || low.starts_with("oss")
        || low.starts_with("bem")
        || low.starts_with("anm")
    {
        "remark"
    } else if low.starts_with("ex")
        || low.starts_with("esem")
        || low.starts_with("beisp")
        || low.starts_with("counter")
        || low.starts_with("contro")
        || low.starts_with("contre")
        || low.starts_with("gegen")
    {
        "example"
    } else {
        "claim"
    }
}

/// Detects whether `text` contains a genuine mathematical statement header (Theorem, Lemma,
/// Proposition, Corollary, Definition, Conjecture, Axiom, Claim) rather than a prose citation.
pub fn detect_math_statement(text: &str) -> Option<DetectedStatement> {
    let trimmed = text.trim();
    if trimmed.len() < 12 {
        return None;
    }

    for cap in statement_header_re().captures_iter(trimmed) {
        let Some(kw_m) = cap.get(1) else {
            continue;
        };
        let kw = kw_m.as_str();
        let num = cap.get(2).map(|m| m.as_str().trim()).filter(|s| !s.is_empty());

        // Check preceding context on the same line/sentence for back-reference prepositions.
        let prefix_start = kw_m.start().saturating_sub(42);
        let mut safe_start = prefix_start;
        while safe_start < kw_m.start() && !trimmed.is_char_boundary(safe_start) {
            safe_start += 1;
        }
        let prefix = &trimmed[safe_start..kw_m.start()];
        let last_line_prefix = prefix.rsplit('\n').next().unwrap_or(prefix);
        if prose_ref_prefix_re().is_match(last_line_prefix) {
            continue;
        }

        // If the keyword appears mid-line right after text that didn't end a sentence, skip.
        let non_ws_before = last_line_prefix.trim_end();
        if !non_ws_before.is_empty()
            && !non_ws_before.ends_with('.')
            && !non_ws_before.ends_with('!')
            && !non_ws_before.ends_with('?')
            && !non_ws_before.ends_with(')')
            && !non_ws_before.ends_with(']')
        {
            continue;
        }

        // Avoid table-of-contents lines like "Theorem 3.2 ........ 45"
        let full_end = cap.get(0).map(|m| m.end()).unwrap_or(kw_m.end());
        let after = trimmed[full_end..].trim_start();
        if after.starts_with("...") || after.starts_with(". . .") {
            continue;
        }

        // Require either a number, or explicit punctuation (`:` / `.`) right after keyword/parenthetical
        // followed by non-empty statement body text.
        if num.is_none() && after.len() < 10 {
            continue;
        }

        let kind = canonical_kind(kw);
        let mut label = String::new();
        let mut chars = kw.chars();
        if let Some(first) = chars.next() {
            for c in first.to_uppercase() {
                label.push(c);
            }
            label.push_str(&chars.as_str().to_lowercase());
        }
        if let Some(n) = num {
            label.push(' ');
            label.push_str(n);
        }

        return Some(DetectedStatement { kind, label });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_multilingual_math_statement_headers() {
        let thm = detect_math_statement(
            "Theorem 3.2 (Riesz Representation). Let H be a Hilbert space and let f be a bounded linear functional.",
        )
        .unwrap();
        assert_eq!(thm.kind, "theorem");
        assert_eq!(thm.label, "Theorem 3.2");

        let lem = detect_math_statement("Some intro sentence.\nLemma 1.4. Suppose X is compact.")
            .unwrap();
        assert_eq!(lem.kind, "lemma");
        assert_eq!(lem.label, "Lemma 1.4");

        let def_it =
            detect_math_statement("Definizione 2.1: Una varietà differenziabile è uno spazio...")
                .unwrap();
        assert_eq!(def_it.kind, "definition");
        assert_eq!(def_it.label, "Definizione 2.1");

        let satz = detect_math_statement("Satz 4. Ist G eine endliche Gruppe...").unwrap();
        assert_eq!(satz.kind, "theorem");
        assert_eq!(satz.label, "Satz 4");

        let rem = detect_math_statement("Osservazione 3.1. Il funzionale non è coercivo su L^1.")
            .unwrap();
        assert_eq!(rem.kind, "remark");
        assert_eq!(rem.label, "Osservazione 3.1");

        let ex = detect_math_statement("Counterexample 2.4. Take f(x) = x sin(1/x) on [0, 1].")
            .unwrap();
        assert_eq!(ex.kind, "example");
        assert_eq!(ex.label, "Counterexample 2.4");
    }

    #[test]
    fn rejects_prose_cross_references_and_toc_lines() {
        assert!(detect_math_statement(
            "By Theorem 3.2 we immediately deduce that the operator is compact."
        )
        .is_none());
        assert!(detect_math_statement(
            "For the proof, see Lemma 1.4 and apply Proposition 2.1 to the boundary."
        )
        .is_none());
        assert!(detect_math_statement(
            "Dal Teorema 3 segue che la serie converge uniformemente."
        )
        .is_none());
        assert!(detect_math_statement("Theorem 3.2 ........... 45").is_none());
    }
}
