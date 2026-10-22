//! Text folding shared by identity keys, sort keys and search (`design/database.md` §3).
//!
//! Identity is a folded key; the displayed name is whatever spelling most files use. Names fold to
//! lowercase, with every `ς` written `σ` and every whitespace character a space, then NFC.
//! Lowercasing is not full Unicode case folding, so `straße` and `STRASSE` stay apart. Sort keys
//! also fold accents away, and search and sort keys spell out the same letters.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Leading articles, which a sort key drops and a search query may leave out.
pub(crate) const ARTICLES: [&str; 3] = ["the", "a", "an"];

/// `name` without a leading article and the whitespace after it, unless the article is all of it.
/// "The Beatles" gives "Beatles"; "The" and "Them" are kept.
pub(crate) fn strip_article(name: &str) -> &str {
    if let Some((first, rest)) = name.split_once(char::is_whitespace)
        && ARTICLES.iter().any(|article| first.eq_ignore_ascii_case(article))
        && !rest.trim_start().is_empty()
    {
        return rest.trim_start();
    }
    name
}

/// `text` with each run of whitespace collapsed to one space and none at either end.
pub(crate) fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` without accents, as a sort key holds it: compatibility-decomposed (NFKD), lowercased,
/// with every diacritic dropped and every `ς` written `σ`, then recomposed (NFC). So `é` is `e`,
/// `й` is `и`, `ＡＢＢＡ` is `abba` and Hangul stays composed. The diacritics are the Combining
/// Diacritical Marks, Hebrew points and cantillation, and Arabic harakat; marks that spell a
/// letter or vowel of another script, such as a Devanagari vowel sign or a Japanese voicing mark,
/// are kept. Letters with no decomposition are written as [`spelled_out`] gives.
pub(crate) fn fold_accents(text: &str) -> String {
    let mut letters = String::with_capacity(text.len());
    for ch in text.nfkd().flat_map(char::to_lowercase).filter(|&ch| !is_diacritic(ch)) {
        match (ch, spelled_out(ch)) {
            (_, Some(spelled)) => letters.push_str(spelled),
            ('ς', None) => letters.push('σ'),
            (ch, None) => letters.push(ch),
        }
    }
    letters.nfc().collect()
}

/// Whether `ch` is a diacritic that sort keys drop (see [`fold_accents`]). Hebrew punctuation
/// among the points, such as the maqaf, is not.
fn is_diacritic(ch: char) -> bool {
    matches!(ch, '\u{300}'..='\u{36f}' | '\u{64b}'..='\u{652}' | '\u{670}')
        || (('\u{591}'..='\u{5c7}').contains(&ch) && is_combining_mark(ch))
}

/// How library filing rules write lowercase `ch` when it is a letter with no decomposition:
/// without its mark (`ø` as `o`) or as two letters (`æ` as `ae`). `None` for every other
/// character. Sort keys and search words share it, so both spell these letters alike.
pub(crate) fn spelled_out(ch: char) -> Option<&'static str> {
    Some(match ch {
        'ø' => "o",
        'ł' => "l",
        'đ' | 'ð' => "d",
        'ı' => "i",
        'æ' => "ae",
        'œ' => "oe",
        'ß' => "ss",
        'þ' => "th",
        _ => return None,
    })
}

/// Fold a name for identity: trim outer whitespace, lowercase, write `ς` as `σ` and each
/// whitespace character (a tab, a no-break space) as a space, then NFC. Normalizing last lets a
/// lowercased base and mark compose (`J\u{30C}` gives `ǰ`). Runs of spaces are kept, so
/// "Sun  Ra", "Sun Ra" and "Sunra" stay distinct.
pub fn name_key(name: &str) -> String {
    name.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .map(|ch| match ch {
            'ς' => 'σ',
            ch if ch.is_whitespace() => ' ',
            ch => ch,
        })
        .nfc()
        .collect()
}

/// Fold a tag for identity (`requirements/tags.md` §4): its name key with repeated spaces
/// collapsed, so "Hip  Hop" is "Hip Hop", while "HipHop" and "Hip-Hop" are other tags.
pub fn tag_key(name: &str) -> String {
    collapse_spaces(&name_key(name))
}

/// The album-artists half of album identity: each artist's `name_key`,
/// sorted so order does not matter, joined with `;`
/// (`design/database.md` §3).
pub fn artists_key<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    let mut keys: Vec<String> = names.into_iter().map(name_key).collect();
    keys.sort();
    keys.dedup();
    keys.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fold_case_not_spacing() {
        assert_eq!(name_key("  Nirvana "), "nirvana");
        assert_eq!(name_key("SUN RA"), "sun ra");
        assert_ne!(name_key("Sun Ra"), name_key("Sunra"));
        assert_eq!(name_key("Björk"), "björk");
        assert_eq!(name_key("BJÖRK"), "björk");
    }

    #[test]
    fn names_fold_canonical_equivalents_and_final_sigma() {
        assert_eq!(name_key("Bj\u{f6}rk"), name_key("Bjo\u{308}rk"));
        assert_eq!(name_key("ΝΊΚΟΣ"), name_key("Νίκος"));
        assert_eq!(name_key("J\u{30C}"), name_key("\u{1F0}"));
        assert_eq!(name_key("\u{399}\u{308}\u{301}"), name_key("\u{390}"));
    }

    #[test]
    fn names_read_any_whitespace_as_a_space_without_collapsing() {
        assert_eq!(name_key("Sun\u{a0}Ra"), name_key("Sun Ra"));
        assert_eq!(name_key("Sun\tRa"), name_key("Sun Ra"));
        assert_ne!(name_key("Sun  Ra"), name_key("Sun Ra"));
        assert_eq!(name_key("\u{a0}Sun Ra\t"), "sun ra");
    }

    #[test]
    fn strip_article_keeps_a_lone_article() {
        assert_eq!(strip_article("The  Beatles"), "Beatles");
        assert_eq!(strip_article("the\tBeatles"), "Beatles");
        assert_eq!(strip_article("The "), "The ");
        assert_eq!(strip_article("Them Crooked Vultures"), "Them Crooked Vultures");
    }

    #[test]
    fn tags_fold_case_and_surrounding_or_repeated_spaces() {
        assert_eq!(tag_key("  Rock "), tag_key("rock"));
        assert_eq!(tag_key("Hip  Hop"), tag_key("Hip Hop"));
        assert_eq!(tag_key("M\u{fa}sica"), tag_key("Mu\u{301}sica"));
    }

    #[test]
    fn tags_merge_nothing_else() {
        assert_ne!(tag_key("Hip Hop"), tag_key("HipHop"));
        assert_ne!(tag_key("Hip Hop"), tag_key("Hip-Hop"));
        assert_ne!(tag_key("HipHop"), tag_key("Hip-Hop"));
        assert_ne!(tag_key("Drum_and_Bass"), tag_key("Drum and Bass"));
        assert_ne!(tag_key("Rock"), tag_key("Rocks"));
    }

    #[test]
    fn artists_key_is_order_independent() {
        assert_eq!(artists_key(["SZA", "Kendrick Lamar"]), artists_key(["kendrick lamar", "sza"]));
        assert_eq!(artists_key(["A", "A"]), "a");
        assert_eq!(artists_key(std::iter::empty()), "");
    }
}
