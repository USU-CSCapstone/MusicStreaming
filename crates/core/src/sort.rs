//! Sort keys (`requirements/conventions.md` §4, `design/general.md` §3).
//!
//! Computed once at scan time and synced. No database collation or client
//! locale ever decides an order, so this function is the only definition of
//! "alphabetical" in Jewelcase and must be byte-identical on every build.

/// Leading articles stripped when no sort tag is present. "The Beatles" files
/// under B.
const ARTICLES: &[&str] = &["the ", "a ", "an "];

/// Build the sort key for a display name.
///
/// A tagged sort value (`ARTISTSORT`, `ALBUMSORT`, ...) wins when present.
/// Otherwise the display name with a leading article removed. The result is
/// case-folded and whitespace-normalized. An empty result means "no sortable
/// value", and callers sort those last with a stable tie-break.
pub fn sort_key(display: &str, sort_tag: Option<&str>) -> String {
    let source = match sort_tag.map(str::trim) {
        Some(s) if !s.is_empty() => s.to_owned(),
        _ => strip_article(display.trim()).to_owned(),
    };
    fold(&source)
}

fn strip_article(name: &str) -> &str {
    let lower = name.to_lowercase();
    for article in ARTICLES {
        if lower.starts_with(article) && lower.len() > article.len() {
            // Byte offsets line up because ARTICLES are ASCII and `to_lowercase`
            // preserves ASCII length for the prefix we compare.
            return name[article.len()..].trim_start();
        }
    }
    name
}

/// Case-fold and collapse whitespace. Deliberately simple: the rule must be
/// reproducible in WebAssembly without locale tables.
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
            last_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn articles_are_stripped() {
        assert_eq!(sort_key("The Beatles", None), "beatles");
        assert_eq!(sort_key("A Tribe Called Quest", None), "tribe called quest");
        assert_eq!(sort_key("An Horse", None), "horse");
    }

    #[test]
    fn a_word_that_is_only_an_article_survives() {
        assert_eq!(sort_key("The", None), "the");
        assert_eq!(sort_key("Them", None), "them");
    }

    #[test]
    fn sort_tag_wins() {
        assert_eq!(
            sort_key("The Beatles", Some("Beatles, The")),
            "beatles, the"
        );
        assert_eq!(sort_key("The Beatles", Some("  ")), "beatles");
    }

    #[test]
    fn whitespace_and_case_are_normalized() {
        assert_eq!(sort_key("  Foo   BAR ", None), "foo bar");
    }
}
