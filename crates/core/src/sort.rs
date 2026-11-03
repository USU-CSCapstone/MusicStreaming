//! Sort keys (`requirements/conventions.md` §4, `design/general.md` §3).
//!
//! Computed once at scan time and synced. No database collation or client
//! locale ever decides an order, so this function is the only definition of
//! "alphabetical" in Jewelcase and must be byte-identical on every build.

use std::cmp::Ordering;

use serde::Deserialize;

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

/// What a track list can be sorted by (`requirements/conventions.md` §4.1). `Added` is when a
/// playlist took the track, so only a playlist's rows carry one. It deserializes from the API's
/// names: `listTracks`' sorts, and `addedToPlaylist`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortField {
    #[serde(rename = "name")]
    Title,
    Artist,
    Album,
    #[serde(rename = "releaseDate")]
    Year,
    #[serde(rename = "addedToPlaylist")]
    Added,
    Duration,
}

/// A track row's keys, everything [`sort_rows`] reads, in the wire's shape: IDs are decimal
/// strings (they pass 2⁵³) and keys are as the API's `sortKeys` give them, so an empty one is
/// unknown.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortRow {
    #[serde(with = "decimal")]
    pub id: u64,
    pub title_key: String,
    pub artist_key: String,
    pub album_key: String,
    #[serde(with = "decimal")]
    pub album_id: u64,
    /// The full release date as text, `None` when the track has none.
    pub release_date: Option<String>,
    /// `None` for a track with no disc, which sorts as disc 0.
    pub disc: Option<u32>,
    /// `None` for a track with no number, which sorts last on its disc.
    pub number: Option<u32>,
    pub duration_us: u64,
    /// When the playlist took the track, as a UTC ISO timestamp; empty outside a playlist.
    #[serde(default)]
    pub added: String,
}

/// An ID the wire spells as a decimal string.
mod decimal {
    use serde::{Deserialize, Deserializer, de::Error};

    /// Reads a decimal string as the ID it spells; fails on anything else.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        String::deserialize(d)?.parse().map_err(Error::custom)
    }
}

impl SortRow {
    /// Album order: album, then disc, then number, then track ID.
    fn album_order(&self, other: &Self) -> Ordering {
        self.album_key
            .cmp(&other.album_key)
            .then(self.album_id.cmp(&other.album_id))
            .then(self.disc.unwrap_or(0).cmp(&other.disc.unwrap_or(0)))
            .then(self.number.unwrap_or(u32::MAX).cmp(&other.number.unwrap_or(u32::MAX)))
            .then(self.id.cmp(&other.id))
    }

    /// Whether the row has no value for `field`, which sorts it last in either direction.
    fn unknown(&self, field: SortField) -> bool {
        match field {
            SortField::Title => self.title_key.is_empty(),
            SortField::Artist => self.artist_key.is_empty(),
            SortField::Album => self.album_key.is_empty(),
            SortField::Year => self.release_date.is_none(),
            SortField::Added | SortField::Duration => false,
        }
    }

    /// The order of two rows by `field`'s own value alone. Keys compare as UTF-8 bytes, which is
    /// code point order.
    fn lead(&self, other: &Self, field: SortField) -> Ordering {
        match field {
            SortField::Title => self.title_key.cmp(&other.title_key),
            SortField::Artist => self.artist_key.cmp(&other.artist_key),
            SortField::Album => self.album_key.cmp(&other.album_key),
            SortField::Year => self.release_date.cmp(&other.release_date),
            SortField::Added => self.added.cmp(&other.added),
            SortField::Duration => self.duration_us.cmp(&other.duration_us),
        }
    }

    /// What breaks a tie on `field`'s value, ending on the track ID. Matches `TrackSort::sort`
    /// on the server.
    fn rest(&self, other: &Self, field: SortField) -> Ordering {
        match field {
            SortField::Artist => self.title_key.cmp(&other.title_key).then(self.id.cmp(&other.id)),
            SortField::Album | SortField::Year => self.album_order(other),
            SortField::Title | SortField::Added | SortField::Duration => self.id.cmp(&other.id),
        }
    }
}

/// The positions of `rows` in `field`'s order, ascending or descending: `rows[result[0]]` is
/// first. Rows with no value for the field sort last in either direction. Ties end on the
/// track ID, then on the row's position in `rows`, which keeps its order in both directions
/// (`design/general.md` §3). Every device and the server therefore agree on one order.
pub fn sort_rows(rows: &[SortRow], field: SortField, descending: bool) -> Vec<usize> {
    let mut positions: Vec<usize> = (0..rows.len()).collect();
    positions.sort_by(|&a, &b| {
        let (x, y) = (&rows[a], &rows[b]);
        let (ux, uy) = (x.unknown(field), y.unknown(field));
        if ux != uy {
            return ux.cmp(&uy);
        }
        let order = x.lead(y, field).then_with(|| x.rest(y, field));
        (if descending { order.reverse() } else { order }).then(a.cmp(&b))
    });
    positions
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

    /// A row with the given ID, title key, artist key, album key and album ID, and the rest
    /// unset.
    fn row(id: u64, title: &str, artist: &str, album: &str, album_id: u64) -> SortRow {
        SortRow {
            id,
            title_key: title.into(),
            artist_key: artist.into(),
            album_key: album.into(),
            album_id,
            release_date: None,
            disc: None,
            number: None,
            duration_us: 1,
            added: String::new(),
        }
    }

    /// The IDs of `rows` in `field`'s order, for comparing a whole order at once.
    fn sorted(rows: &[SortRow], field: SortField, descending: bool) -> Vec<u64> {
        sort_rows(rows, field, descending).into_iter().map(|p| rows[p].id).collect()
    }

    #[test]
    fn titles_sort_with_unknown_last_either_way() {
        let rows = [
            row(1, "", "", "", 1),
            row(2, "b", "", "", 1),
            row(3, "a", "", "", 1),
            row(4, "", "", "", 1),
            row(5, "b", "", "", 1),
        ];
        assert_eq!(sorted(&rows, SortField::Title, false), [3, 2, 5, 1, 4]);
        // Descending reverses known rows and the unknown rows' ties alike, unknown still last.
        assert_eq!(sorted(&rows, SortField::Title, true), [5, 2, 3, 4, 1]);
    }

    #[test]
    fn keys_compare_by_code_point() {
        // U+FFFD sorts before U+1F3B5 by code point, though not by UTF-16 code units.
        let rows = [row(1, "\u{1f3b5}", "", "", 1), row(2, "\u{fffd}", "", "", 1)];
        assert_eq!(sorted(&rows, SortField::Title, false), [2, 1]);
    }

    #[test]
    fn ids_compare_as_numbers() {
        let rows = [row(10, "a", "", "", 1), row(9, "a", "", "", 1), row(u64::MAX, "a", "", "", 1)];
        assert_eq!(sorted(&rows, SortField::Title, false), [9, 10, u64::MAX]);
    }

    #[test]
    fn artists_break_ties_on_title_then_id() {
        let rows = [
            row(1, "b", "x", "", 1),
            row(2, "a", "x", "", 1),
            row(3, "a", "", "", 1),
            row(4, "a", "w", "", 1),
            row(5, "a", "x", "", 1),
            row(6, "0", "", "", 1),
        ];
        assert_eq!(sorted(&rows, SortField::Artist, false), [4, 2, 5, 1, 6, 3]);
        assert_eq!(sorted(&rows, SortField::Artist, true), [1, 5, 2, 4, 3, 6]);
    }

    #[test]
    fn albums_break_ties_on_album_disc_number() {
        let mut rows = [
            row(1, "", "", "kid a", 2),
            row(2, "", "", "abbey", 1),
            row(3, "", "", "abbey", 1),
            row(4, "", "", "abbey", 1),
            row(5, "", "", "abbey", 1),
            row(6, "", "", "", 3),
            row(7, "", "", "abbey", 0),
        ];
        rows[1].disc = Some(2);
        rows[1].number = Some(1);
        rows[2].disc = Some(1);
        rows[2].number = Some(2);
        // No disc is disc 0, and no number is last on its disc.
        rows[3].number = Some(1);
        rows[4].disc = Some(1);
        assert_eq!(sorted(&rows, SortField::Album, false), [7, 4, 3, 5, 2, 1, 6]);
        assert_eq!(sorted(&rows, SortField::Album, true), [1, 2, 5, 3, 4, 7, 6]);
    }

    #[test]
    fn years_are_full_dates_with_undated_last() {
        let mut rows = [
            row(1, "", "", "b", 2),
            row(2, "", "", "a", 1),
            row(3, "", "", "b", 1),
            row(4, "", "", "a", 3),
        ];
        rows[0].release_date = Some("1999-05".into());
        rows[1].release_date = Some("1999".into());
        rows[2].release_date = Some("1999".into());
        // Undated rows fall back to album order.
        assert_eq!(sorted(&rows, SortField::Year, false), [2, 3, 1, 4]);
        assert_eq!(sorted(&rows, SortField::Year, true), [1, 3, 2, 4]);
    }

    #[test]
    fn durations_tie_on_id() {
        let mut rows = [row(3, "", "", "", 1), row(1, "", "", "", 1), row(2, "", "", "", 1)];
        rows[0].duration_us = 5;
        rows[1].duration_us = 9;
        rows[2].duration_us = 5;
        assert_eq!(sorted(&rows, SortField::Duration, false), [2, 3, 1]);
        assert_eq!(sorted(&rows, SortField::Duration, true), [1, 3, 2]);
    }

    #[test]
    fn added_ties_end_on_id_then_position() {
        let mut rows = [
            row(7, "", "", "", 1),
            row(3, "", "", "", 1),
            row(7, "", "", "", 1),
            row(5, "", "", "", 1),
        ];
        for (r, added) in rows.iter_mut().zip(["02", "02", "02", "01"]) {
            r.added = added.into();
        }
        // The track held twice keeps the playlist's order in both directions.
        let positions = |descending| sort_rows(&rows, SortField::Added, descending);
        assert_eq!(positions(false), [3, 1, 0, 2]);
        assert_eq!(positions(true), [0, 2, 1, 3]);
    }

    #[test]
    fn rows_come_from_the_wire() {
        let rows: Vec<SortRow> = serde_json::from_str(
            r#"[{"id":"18446744073709551615","titleKey":"a","artistKey":"","albumKey":"",
                "albumId":"7","releaseDate":null,"disc":null,"number":3,"durationUs":4}]"#,
        )
        .unwrap();
        assert_eq!((rows[0].id, rows[0].album_id, rows[0].number), (u64::MAX, 7, Some(3)));
        assert!(serde_json::from_str::<Vec<SortRow>>(r#"[{"id":"x"}]"#).is_err());
    }

    /// The cases in `queue/parity.json`, which the web build's `core-wasm.spec.ts` checks in
    /// WebAssembly too.
    #[test]
    fn sort_golden_cases() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("queue/parity.json")).unwrap();
        let rows: Vec<SortRow> = serde_json::from_value(fixture["sort"]["rows"].clone()).unwrap();
        for case in fixture["sort"]["cases"].as_array().unwrap() {
            let field: SortField = serde_json::from_value(case["field"].clone()).unwrap();
            let descending = case["order"] == "desc";
            let want: Vec<usize> = serde_json::from_value(case["positions"].clone()).unwrap();
            assert_eq!(sort_rows(&rows, field, descending), want, "{case}");
        }
    }
}
