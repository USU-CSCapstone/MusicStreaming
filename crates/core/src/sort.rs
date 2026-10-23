//! Sort keys (`requirements/conventions.md` §4, `design/general.md` §3).
//!
//! Computed once at scan time and synced. No database collation or client
//! locale ever decides an order, so this function is the only definition of
//! "alphabetical" in Jewelcase and must be byte-identical on every build.

use crate::fold::{collapse_spaces, fold_accents, name_key, strip_article};

/// Build the sort key for a display name.
///
/// A tagged sort value (`ARTISTSORT`, `ALBUMSORT`, ...) wins when present.
/// Otherwise the display name with a leading article removed, so "The Beatles"
/// files under B. The result is folded as a name key is, with compatibility
/// forms and accents folded away and whitespace collapsed, so "Édith Piaf"
/// files under E. Names that differ only in accents share a key, and callers
/// break that tie by ID. An empty result means "no sortable value", and callers
/// sort those last with a stable tie-break.
pub fn sort_key(display: &str, sort_tag: Option<&str>) -> String {
    let source = match sort_tag.map(str::trim) {
        Some(tag) if !tag.is_empty() => tag,
        _ => strip_article(display.trim()),
    };
    tag_sort_key(source)
}

/// Build the sort key for a tag, such as a genre: its name folded as [`sort_key`] folds one,
/// with any leading article kept, so "A Cappella" files under A.
pub fn tag_sort_key(name: &str) -> String {
    collapse_spaces(&fold_accents(&name_key(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn articles_are_stripped() {
        assert_eq!(sort_key("The Beatles", None), "beatles");
        assert_eq!(sort_key("A Tribe Called Quest", None), "tribe called quest");
        assert_eq!(sort_key("An Horse", None), "horse");
        assert_eq!(sort_key("The\tBeatles", None), "beatles");
        assert_eq!(sort_key("THE  BEATLES", None), "beatles");
    }

    #[test]
    fn tags_keep_their_articles() {
        assert_eq!(tag_sort_key("A Cappella"), "a cappella");
        assert_eq!(tag_sort_key("  The  Blues "), "the blues");
        assert_eq!(tag_sort_key("Électro"), "electro");
    }

    #[test]
    fn a_word_that_is_only_an_article_survives() {
        assert_eq!(sort_key("The", None), "the");
        assert_eq!(sort_key("Them", None), "them");
    }

    #[test]
    fn sort_tag_wins() {
        assert_eq!(sort_key("The Beatles", Some("Beatles, The")), "beatles, the");
        assert_eq!(sort_key("The Beatles", Some("  ")), "beatles");
    }

    #[test]
    fn whitespace_and_case_are_normalized() {
        assert_eq!(sort_key("  Foo   BAR ", None), "foo bar");
    }

    #[test]
    fn accents_are_ignored() {
        assert_eq!(sort_key("Édith Piaf", None), "edith piaf");
        assert_eq!(sort_key("E\u{301}dith Piaf", None), "edith piaf");
        assert_eq!(sort_key("Édith Piaf", Some("Piaf, Édith")), "piaf, edith");
        assert_eq!(sort_key("Ångström", None), "angstrom");
        assert_eq!(sort_key("Phở Bò", None), "pho bo");
        assert_eq!(sort_key("Мумий Тролль", None), "мумии тролль");
        assert_eq!(sort_key("ῶ", None), "ω");
        assert_eq!(sort_key("Ά", None), "α");
        let order = ["Dylan", "Edith Piaf", "Édith Piaf", "Elvis", "Zappa"];
        assert!(order.windows(2).all(|w| sort_key(w[0], None) <= sort_key(w[1], None)));
        // Names differing only in accents tie; lists break the tie by ID.
        assert_eq!(sort_key("Édith Piaf", None), sort_key("Edith Piaf", None));
    }

    #[test]
    fn letters_without_a_decomposition_file_as_spelled_without_their_mark() {
        assert_eq!(sort_key("Øystein", None), "oystein");
        assert_eq!(sort_key("Łódź", None), "lodz");
        assert_eq!(sort_key("Ærø", None), "aero");
        assert_eq!(sort_key("Straße", None), "strasse");
        assert_eq!(sort_key("Œuvre", None), "oeuvre");
        assert_eq!(sort_key("Þórir", None), "thorir");
        assert_eq!(sort_key("Đorđe", None), "dorde");
        assert_eq!(sort_key("Guðrún", None), "gudrun");
        assert_eq!(sort_key("Sıla", None), "sila");
        for (before, name, after) in [
            ("Anathema", "Ångström", "Anthrax"),
            ("Oasis", "Øystein", "Ozzy"),
            ("Lizzo", "Łódź", "Lorde"),
            ("Adele", "Ærø", "Afghan Whigs"),
        ] {
            assert!(sort_key(before, None) < sort_key(name, None), "{name}");
            assert!(sort_key(name, None) < sort_key(after, None), "{name}");
        }
    }

    #[test]
    fn hebrew_points_and_arabic_harakat_are_ignored() {
        assert_eq!(sort_key("שָׁלוֹם", None), sort_key("שלום", None));
        assert_eq!(sort_key("\u{5d0}\u{591}\u{5c7}\u{5d1}", None), "\u{5d0}\u{5d1}");
        assert_eq!(sort_key("מָה־זֶּה", None), "מה\u{5be}זה");
        assert_eq!(sort_key("\u{5d0}\u{5c0}\u{5c3}\u{5c6}", None), "\u{5d0}\u{5c0}\u{5c3}\u{5c6}");
        assert_eq!(sort_key("مُحَمَّد", None), sort_key("محمد", None));
        assert_eq!(sort_key("\u{627}\u{64b}\u{670}\u{652}", None), "\u{627}");
        assert_eq!(sort_key("\u{627}\u{653}", None), "\u{622}");
    }

    #[test]
    fn compatibility_forms_fold_and_spaces_collapse_after_folding() {
        assert_eq!(sort_key("ＡＢＢＡ", None), "abba");
        assert_eq!(sort_key("\u{fb01}re", None), "fire");
        assert_eq!(sort_key("Hello™", None), "hellotm");
        assert_eq!(sort_key("\u{1d6d3}", None), "σ");
        assert_eq!(sort_key("\u{301} Edith", None), "edith");
        assert_eq!(sort_key("Sun \u{301} Ra", None), "sun ra");
    }

    #[test]
    fn marks_that_spell_other_scripts_are_kept() {
        assert_ne!(sort_key("कि", None), sort_key("का", None));
        assert_ne!(sort_key("が", None), sort_key("か", None));
        assert_eq!(sort_key("한국", None), "한국");
    }

    #[test]
    fn canonical_equivalents_and_final_sigma_sort_alike() {
        assert_eq!(sort_key("Bj\u{f6}rk", None), sort_key("Bjo\u{308}rk", None));
        assert_eq!(sort_key("ΝΊΚΟΣ", None), sort_key("Νίκος", None));
    }
}
