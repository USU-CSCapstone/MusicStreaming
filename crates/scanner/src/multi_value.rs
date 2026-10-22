//! Multi-value tag splitting (`requirements/artists.md` §1, `requirements/tags.md` §3).
//!
//! The semicolon is the only delimiter. Nothing here splits on "feat.", "&",
//! "vs." or commas: "Earth, Wind & Fire" is one artist.

use jewelcase_core::name_key;

/// Split raw tag values, one per field the tag carried natively, into one ordered list. Each
/// value is trimmed and empties are dropped. A value with the same name key as an earlier one is
/// dropped too, so the first spelling stays.
pub fn split_all<'a>(raws: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut keys = Vec::new();
    let values = raws.into_iter().flat_map(|raw| raw.split(';')).map(str::trim);
    for value in values.filter(|value| !value.is_empty()) {
        let key = name_key(value);
        if !keys.contains(&key) {
            keys.push(key);
            out.push(value.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semicolon_is_the_only_delimiter() {
        assert_eq!(split_all(["Kendrick Lamar;SZA"]), vec!["Kendrick Lamar", "SZA"]);
        assert_eq!(split_all(["Earth, Wind & Fire"]), vec!["Earth, Wind & Fire"]);
        assert_eq!(split_all(["A feat. B"]), vec!["A feat. B"]);
    }

    #[test]
    fn trims_and_drops_empties() {
        assert_eq!(split_all([" A ; ;B;"]), vec!["A", "B"]);
        assert!(split_all(["  ;  "]).is_empty());
    }

    #[test]
    fn native_repeats_and_semicolons_read_the_same() {
        assert_eq!(split_all(["A", "B"]), split_all(["A;B"]));
        assert_eq!(split_all(["A;B", "B", "C"]), vec!["A", "B", "C"]);
    }

    #[test]
    fn repeats_differing_only_in_case_keep_the_first_spelling() {
        assert_eq!(split_all(["A;a"]), vec!["A"]);
        assert_eq!(split_all(["Sun Ra;Sun  Ra"]), vec!["Sun Ra", "Sun  Ra"]);
        assert_eq!(
            split_all(["Kendrick Lamar", "kendrick lamar;SZA"]),
            vec!["Kendrick Lamar", "SZA"]
        );
    }
}
