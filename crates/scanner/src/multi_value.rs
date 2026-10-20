//! Multi-value tag splitting (`requirements/artists.md` §1, `requirements/tags.md` §3).
//!
//! The semicolon is the only delimiter. Nothing here splits on "feat.", "&",
//! "vs." or commas: "Earth, Wind & Fire" is one artist.

/// Split a raw tag value into its values. Trims whitespace, drops empties,
/// preserves order, and de-duplicates exact repeats.
pub fn split(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if !out.iter().any(|existing| existing == part) {
            out.push(part.to_owned());
        }
    }
    out
}

/// Split many raw values (a tag that carried repeated fields natively) and
/// merge them into one ordered list.
pub fn split_all<'a>(raws: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in raws {
        for value in split(raw) {
            if !out.contains(&value) {
                out.push(value);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semicolon_is_the_only_delimiter() {
        assert_eq!(split("Kendrick Lamar;SZA"), vec!["Kendrick Lamar", "SZA"]);
        assert_eq!(split("Earth, Wind & Fire"), vec!["Earth, Wind & Fire"]);
        assert_eq!(split("A feat. B"), vec!["A feat. B"]);
    }

    #[test]
    fn trims_and_drops_empties() {
        assert_eq!(split(" A ; ;B;"), vec!["A", "B"]);
        assert!(split("  ;  ").is_empty());
    }

    #[test]
    fn native_repeats_and_semicolons_read_the_same() {
        assert_eq!(split_all(["A", "B"]), split("A;B"));
        assert_eq!(split_all(["A;B", "B", "C"]), vec!["A", "B", "C"]);
    }
}
