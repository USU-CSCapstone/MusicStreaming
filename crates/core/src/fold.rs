//! Text folding shared by identity keys, sort keys and search (`design/database.md` §3).
//!
//! Identity is a folded key; the displayed name is whatever spelling most files use. Names fold to
//! lowercase, with every `ς` written `σ` and every whitespace character a space, then NFC.
//! Lowercasing is not full Unicode case folding, so `straße` and `STRASSE` stay apart.

use unicode_normalization::UnicodeNormalization;

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
