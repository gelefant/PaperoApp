use crate::metadata::models::normalize_pages;

/// Escapes Unicode diacritics to AMS/MRLookup BibTeX LaTeX macros.
pub fn unicode_to_ams_latex(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        // Handle decomposed combining sequence if base + combining mark
        if i + 1 < chars.len() {
            if let Some(macro_str) = combine_pair_to_latex(ch, chars[i + 1]) {
                out.push_str(macro_str);
                i += 2;
                continue;
            }
        }
        match ch {
            // Acute
            'á' => out.push_str(r"{\'a}"),
            'Á' => out.push_str(r"{\'A}"),
            'é' => out.push_str(r"{\'e}"),
            'É' => out.push_str(r"{\'E}"),
            'í' => out.push_str(r"{\'{\i}}"),
            'Í' => out.push_str(r"{\'I}"),
            'ó' => out.push_str(r"{\'o}"),
            'Ó' => out.push_str(r"{\'O}"),
            'ú' => out.push_str(r"{\'u}"),
            'Ú' => out.push_str(r"{\'U}"),
            'ý' => out.push_str(r"{\'y}"),
            'Ý' => out.push_str(r"{\'Y}"),
            'ć' => out.push_str(r"{\'c}"),
            'Ć' => out.push_str(r"{\'C}"),
            'ń' => out.push_str(r"{\'n}"),
            'Ń' => out.push_str(r"{\'N}"),
            'ś' => out.push_str(r"{\'s}"),
            'Ś' => out.push_str(r"{\'S}"),
            'ź' => out.push_str(r"{\'z}"),
            'Ź' => out.push_str(r"{\'Z}"),
            // Grave
            'à' => out.push_str(r"{\`a}"),
            'À' => out.push_str(r"{\`A}"),
            'è' => out.push_str(r"{\`e}"),
            'È' => out.push_str(r"{\`E}"),
            'ì' => out.push_str(r"{\`{\i}}"),
            'Ì' => out.push_str(r"{\`I}"),
            'ò' => out.push_str(r"{\`o}"),
            'Ò' => out.push_str(r"{\`O}"),
            'ù' => out.push_str(r"{\`u}"),
            'Ù' => out.push_str(r"{\`U}"),
            // Circumflex
            'â' => out.push_str(r"{\^a}"),
            'Â' => out.push_str(r"{\^A}"),
            'ê' => out.push_str(r"{\^e}"),
            'Ê' => out.push_str(r"{\^E}"),
            'î' => out.push_str(r"{\^{\i}}"),
            'Î' => out.push_str(r"{\^I}"),
            'ô' => out.push_str(r"{\^o}"),
            'Ô' => out.push_str(r"{\^O}"),
            'û' => out.push_str(r"{\^u}"),
            'Û' => out.push_str(r"{\^U}"),
            // Diaeresis / Umlaut
            'ä' => out.push_str(r#"{\"a}"#),
            'Ä' => out.push_str(r#"{\"A}"#),
            'ë' => out.push_str(r#"{\"e}"#),
            'Ë' => out.push_str(r#"{\"E}"#),
            'ï' => out.push_str(r#"{\"{\i}}"#),
            'Ï' => out.push_str(r#"{\"I}"#),
            'ö' => out.push_str(r#"{\"o}"#),
            'Ö' => out.push_str(r#"{\"O}"#),
            'ü' => out.push_str(r#"{\"u}"#),
            'Ü' => out.push_str(r#"{\"U}"#),
            'ÿ' => out.push_str(r#"{\"y}"#),
            // Double acute (Erdős!)
            'ő' => out.push_str(r"{\H{o}}"),
            'Ő' => out.push_str(r"{\H{O}}"),
            'ű' => out.push_str(r"{\H{u}}"),
            'Ű' => out.push_str(r"{\H{U}}"),
            // Caron / Haceck (Čech!)
            'č' => out.push_str(r"{\v{c}}"),
            'Č' => out.push_str(r"{\v{C}}"),
            'š' => out.push_str(r"{\v{s}}"),
            'Š' => out.push_str(r"{\v{S}}"),
            'ž' => out.push_str(r"{\v{z}}"),
            'Ž' => out.push_str(r"{\v{Z}}"),
            'ř' => out.push_str(r"{\v{r}}"),
            'Ř' => out.push_str(r"{\v{R}}"),
            'ě' => out.push_str(r"{\v{e}}"),
            'Ě' => out.push_str(r"{\v{E}}"),
            // Cedilla / Comma below
            'ç' => out.push_str(r"{\c{c}}"),
            'Ç' => out.push_str(r"{\c{C}}"),
            'ş' | 'ș' => out.push_str(r"{\c{s}}"),
            'Ş' | 'Ș' => out.push_str(r"{\c{S}}"),
            'ţ' | 'ț' => out.push_str(r"{\c{t}}"),
            'Ţ' | 'Ț' => out.push_str(r"{\c{T}}"),
            // Tilde / Ring / Bar / Eszett
            'ñ' => out.push_str(r"{\~n}"),
            'Ñ' => out.push_str(r"{\~N}"),
            'ã' => out.push_str(r"{\~a}"),
            'Ã' => out.push_str(r"{\~A}"),
            'å' => out.push_str(r"{\aa}"),
            'Å' => out.push_str(r"{\AA}"),
            'ł' => out.push_str(r"{\l{}}"),
            'Ł' => out.push_str(r"{\L{}}"),
            'ø' => out.push_str(r"{\o{}}"),
            'Ø' => out.push_str(r"{\O{}}"),
            'ß' => out.push_str(r"{\ss}"),
            'æ' => out.push_str(r"{\ae}"),
            'Æ' => out.push_str(r"{\AE}"),
            'œ' => out.push_str(r"{\oe}"),
            'Œ' => out.push_str(r"{\OE}"),
            '–' => out.push_str("--"),
            '—' => out.push_str("---"),
            '’' | '‘' => out.push('\''),
            '“' => out.push_str("``"),
            '”' => out.push_str("''"),
            _ => out.push(ch),
        }
        i += 1;
    }
    out
}

fn combine_pair_to_latex(base: char, comb: char) -> Option<&'static str> {
    match (base, comb) {
        ('a', '\u{0301}') => Some(r"{\'a}"),
        ('e', '\u{0301}') => Some(r"{\'e}"),
        ('o', '\u{0301}') => Some(r"{\'o}"),
        ('u', '\u{0301}') => Some(r"{\'u}"),
        ('a', '\u{0308}') => Some(r#"{\"a}"#),
        ('o', '\u{0308}') => Some(r#"{\"o}"#),
        ('u', '\u{0308}') => Some(r#"{\"u}"#),
        ('o', '\u{030B}') => Some(r"{\H{o}}"),
        ('c', '\u{030C}') => Some(r"{\v{c}}"),
        ('C', '\u{030C}') => Some(r"{\v{C}}"),
        ('s', '\u{0327}') => Some(r"{\c{s}}"),
        _ => None,
    }
}

const MATH_PROPER_NOUNS: &[(&str, &str)] = &[
    ("Abel", "{A}bel"),
    ("Banach", "{B}anach"),
    ("Bernstein", "{B}ernstein"),
    ("Bézout", "{B}{\\'e}zout"),
    ("Borel", "{B}orel"),
    ("Calabi", "{C}alabi"),
    ("Cantor", "{C}antor"),
    ("Cartan", "{C}artan"),
    ("Cauchy", "{C}auchy"),
    ("Čech", "{\\v{C}}ech"),
    ("Chern", "{C}hern"),
    ("Dirichlet", "{D}irichlet"),
    ("Erdős", "{E}rd{\\H{o}}s"),
    ("Euler", "{E}uler"),
    ("Fermat", "{F}ermat"),
    ("Fourier", "{F}ourier"),
    ("Frobenius", "{F}robenius"),
    ("Galois", "{G}alois"),
    ("Gauss", "{G}auss"),
    ("Gaussian", "{G}aussian"),
    ("Gödel", "{G}{\\\"o}del"),
    ("Green", "{G}reen"),
    ("Grothendieck", "{G}rothendieck"),
    ("Hadamard", "{H}adamard"),
    ("Hamilton", "{H}amilton"),
    ("Hausdorff", "{H}ausdorff"),
    ("Hermite", "{H}ermite"),
    ("Hilbert", "{H}ilbert"),
    ("Hodge", "{H}odge"),
    ("Hölder", "{H}{\\\"o}lder"),
    ("Jacobi", "{J}acobi"),
    ("Kähler", "{K}{\\\"a}hler"),
    ("Kolmogorov", "{K}olmogorov"),
    ("Laplace", "{L}aplace"),
    ("Lebesgue", "{L}ebesgue"),
    ("Legendre", "{L}egendre"),
    ("Lie", "{L}ie"),
    ("Lipschitz", "{L}ipschitz"),
    ("Markov", "{M}arkov"),
    ("Milnor", "{M}ilnor"),
    ("Minkowski", "{M}inkowski"),
    ("Möbius", "{M}{\\\"o}bius"),
    ("Navier", "{N}avier"),
    ("Newton", "{N}ewton"),
    ("Noether", "{N}oether"),
    ("Poincaré", "{P}oincar{\\'e}"),
    ("Poisson", "{P}oisson"),
    ("Rham", "{R}ham"),
    ("Ricci", "{R}icci"),
    ("Riemann", "{R}iemann"),
    ("Riemannian", "{R}iemannian"),
    ("Schrödinger", "{S}chr{\\\"o}dinger"),
    ("Sobolev", "{S}obolev"),
    ("Stokes", "{S}tokes"),
    ("Turing", "{T}uring"),
    ("Weil", "{W}eil"),
    ("Weyl", "{W}eyl"),
    ("Yang", "{Y}ang"),
    ("Yau", "{Y}au"),
    ("Zariski", "{Z}ariski"),
];

/// Formats a plain Unicode title for reconstructed MRLookup-style BibTeX:
/// protects non-initial capitalized proper nouns/acronyms (`{R}ham`, `{PDE}`, `L^p`) and
/// converts diacritics to AMS LaTeX macros. Does not double-brace if braces are already present.
pub fn format_bibtex_title(raw_title: &str) -> String {
    if raw_title.contains('{') || raw_title.contains('\\') {
        return unicode_to_ams_latex(raw_title);
    }

    let words: Vec<&str> = raw_title.split_whitespace().collect();
    let mut out = Vec::with_capacity(words.len());
    for (idx, word) in words.iter().enumerate() {
        let stripped = word.trim_matches(|c: char| !c.is_alphanumeric() && c != 'é' && c != 'ä' && c != 'ö' && c != 'ő' && c != 'Č');
        if let Some((_, repl)) = MATH_PROPER_NOUNS.iter().find(|(k, _)| *k == stripped) {
            out.push(word.replacen(stripped, repl, 1));
            continue;
        }
        let escaped = unicode_to_ams_latex(word);
        // Protect internal acronyms or capitalized non-initial proper names
        let is_acronym = word.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 2
            && word.chars().all(|c| !c.is_lowercase());
        if is_acronym {
            out.push(format!("{{{escaped}}}"));
        } else if idx > 0 {
            let mut cs = escaped.chars();
            if let Some(first) = cs.next() {
                if first.is_ascii_uppercase() {
                    out.push(format!("{{{first}}}{}", cs.as_str()));
                    continue;
                }
            }
            out.push(escaped);
        } else {
            out.push(escaped);
        }
    }
    out.join(" ")
}

pub fn format_bibtex_pages(raw: &str) -> String {
    normalize_pages(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_diacritics_and_protects_proper_nouns() {
        assert_eq!(
            unicode_to_ams_latex("Constantin Udriște & Paul Erdős"),
            r"Constantin Udri{\c{s}}te & Paul Erd{\H{o}}s"
        );
        assert_eq!(
            format_bibtex_title("On de Rham cohomology and PDE on Riemannian manifolds"),
            "On de {R}ham cohomology and {PDE} on {R}iemannian manifolds"
        );
        assert_eq!(format_bibtex_pages("577-685"), "577--685");
    }
}
