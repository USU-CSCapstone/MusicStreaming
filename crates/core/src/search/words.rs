//! Folding a name into the words search compares (the rules are in the module above).

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Whether `ch` is left out of a word without ending it: a combining mark or an apostrophe.
fn is_dropped(ch: char) -> bool {
    is_combining_mark(ch) || ch == '\'' || ch == '\u{2019}'
}

/// The folded words of `text`.
pub fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    // A symbol becomes a space before decomposing, so `™` separates rather than adding `tm`.
    let kept = text.chars().map(|ch| if ch.is_alphanumeric() || is_dropped(ch) { ch } else { ' ' });
    for ch in kept.nfkd() {
        if is_dropped(ch) {
            continue;
        }
        if ch.is_alphanumeric() {
            match ch {
                // Letters with no decomposition that people type without their mark.
                'ß' | 'ẞ' => word.push_str("ss"),
                'æ' | 'Æ' => word.push_str("ae"),
                'œ' | 'Œ' => word.push_str("oe"),
                'ø' | 'Ø' => word.push('o'),
                'ł' | 'Ł' => word.push('l'),
                'đ' | 'Đ' => word.push('d'),
                'þ' | 'Þ' => word.push_str("th"),
                // `Σ` lowers to `σ` wherever it stands, so final `ς` folds to `σ` too.
                'ς' => word.push('σ'),
                _ => word.extend(ch.to_lowercase()),
            }
        } else if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_accents_case_and_apostrophes() {
        assert_eq!(words("Björk"), ["bjork"]);
        assert_eq!(words("SAY it Ain’t so!"), ["say", "it", "aint", "so"]);
        assert_eq!(words("AC/DC — Live"), ["ac", "dc", "live"]);
        assert_eq!(words("Straße Øresund Ærø"), ["strasse", "oresund", "aero"]);
        assert_eq!(words("STRAẞE"), ["strasse"]);
        assert_eq!(words("  ...  "), Vec::<String>::new());
    }

    #[test]
    fn folds_compatibility_forms() {
        assert_eq!(words("\u{FB01}re"), ["fire"]);
        assert_eq!(words("ＡＢＢＡ ２"), ["abba", "2"]);
        assert_eq!(words("½"), ["1", "2"]);
    }

    #[test]
    fn symbols_separate_words() {
        assert_eq!(words("Hello™"), ["hello"]);
        assert_eq!(words("№5"), ["5"]);
    }

    #[test]
    fn folds_final_sigma() {
        assert_eq!(words("ΝΙΚΟΣ"), ["νικοσ"]);
        assert_eq!(words("Νίκος"), ["νικοσ"]);
    }
}
