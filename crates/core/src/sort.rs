//! Sort keys (`requirements/conventions.md` §4, `design/general.md` §3).
//!
//! Computed once at scan time and synced. No database collation or client
//! locale ever decides an order, so this function is the only definition of
//! "alphabetical" in Jewelcase and must be byte-identical on every build.

use crate::fold::{collapse_spaces, name_key, strip_article};

/// Build the sort key for a display name.
///
/// A tagged sort value (`ARTISTSORT`, `ALBUMSORT`, ...) wins when present.
/// Otherwise the display name with a leading article removed, so "The Beatles"
/// files under B. The result is folded as a name key is, with whitespace
/// collapsed. An empty result means "no sortable value", and callers sort those
/// last with a stable tie-break.
pub fn sort_key(display: &str, sort_tag: Option<&str>) -> String {
    let source = match sort_tag.map(str::trim) {
        Some(tag) if !tag.is_empty() => tag,
        _ => strip_article(display.trim()),
    };
    collapse_spaces(&name_key(source))
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
    fn canonical_equivalents_and_final_sigma_sort_alike() {
        assert_eq!(sort_key("Bj\u{f6}rk", None), sort_key("Bjo\u{308}rk", None));
        assert_eq!(sort_key("ΝΊΚΟΣ", None), sort_key("Νίκος", None));
    }
}
