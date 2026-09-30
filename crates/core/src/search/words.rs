//! Folding a name into the words search compares (the rules are in the module above).

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// The folded words of `text`.
pub fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for ch in text.nfd() {
        if is_combining_mark(ch) || ch == '\'' || ch == '\u{2019}' {
            continue;
        }
        if ch.is_alphanumeric() {
            match ch {
                // Letters with no decomposition that people type without their mark.
                'ß' => word.push_str("ss"),
                'æ' | 'Æ' => word.push_str("ae"),
                'œ' | 'Œ' => word.push_str("oe"),
                'ø' | 'Ø' => word.push('o'),
                'ł' | 'Ł' => word.push('l'),
                'đ' | 'Đ' => word.push('d'),
                'þ' | 'Þ' => word.push_str("th"),
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
        assert_eq!(words("  ...  "), Vec::<String>::new());
    }
}
