//! The canonical tag set (`requirements/scanning.md` §2, `design/scanning.md` §4).
//!
//! Whatever the tagging scheme, the scanner produces exactly this. Two files
//! carrying the same logical metadata in ID3v2 and Vorbis comments yield
//! equal `TagSet`s; that equality is what the golden-corpus test asserts.

use crate::lrc::{self, SyncedLine};

/// A date at whatever precision the tag carried (`requirements/tracks.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PartialDate {
    pub year: u16,
    pub month: Option<u8>,
    pub day: Option<u8>,
}

impl PartialDate {
    /// Parse `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, or any of those followed by a
    /// time. Anything else is `None`; a date is never guessed.
    pub fn parse(raw: &str) -> Option<PartialDate> {
        let raw = raw.trim();
        let date_part = raw.split(['T', ' ']).next()?;
        let mut parts = date_part.split('-');
        let year: u16 = parts.next()?.parse().ok().filter(|y| (1000..=9999).contains(y))?;
        let month = match parts.next() {
            Some(m) => Some(m.parse::<u8>().ok().filter(|m| (1..=12).contains(m))?),
            None => None,
        };
        let day = match (month, parts.next()) {
            (Some(_), Some(d)) => Some(d.parse::<u8>().ok().filter(|d| (1..=31).contains(d))?),
            _ => None,
        };
        Some(PartialDate { year, month, day })
    }
}

impl std::fmt::Display for PartialDate {
    /// `YYYY`, `YYYY-MM`, or `YYYY-MM-DD`: the shape the database and API
    /// carry.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}", self.year)?;
        if let Some(m) = self.month {
            write!(f, "-{m:02}")?;
            if let Some(d) = self.day {
                write!(f, "-{d:02}")?;
            }
        }
        Ok(())
    }
}

/// Lyrics as read from the file or a sidecar, in the one form they are stored and served
/// (`requirements/tracks.md` §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lyrics {
    /// Text with no timed lines.
    Plain(String),
    /// The timed lines, sorted by time.
    Synced(Vec<SyncedLine>),
}

impl Lyrics {
    /// Lyrics from text, less any leading byte order mark: synced when the LRC parser finds a
    /// timed line anywhere, plain otherwise. Plain text loses surrounding whitespace. `None` when
    /// nothing is left.
    pub fn parse(text: &str) -> Option<Lyrics> {
        let text = text.trim_start_matches('\u{FEFF}');
        let lines = lrc::parse(text);
        if !lines.is_empty() {
            return Some(Lyrics::Synced(lines));
        }
        let text = text.trim();
        (!text.is_empty()).then(|| Lyrics::Plain(text.to_owned()))
    }
}

/// Everything Jewelcase reads from a file's tags. Every field is optional or
/// empty by default; an untagged file is a valid `TagSet`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TagSet {
    /// Track title. `None` means the scanner names the track after its file.
    pub title: Option<String>,
    /// Ordered track artists; the first is the primary credit.
    pub artists: Vec<String>,
    pub album: Option<String>,
    /// Ordered album artists. Participate in album identity
    /// (`requirements/albums.md` §1).
    pub album_artists: Vec<String>,
    pub compilation: bool,
    pub track_number: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_number: Option<u32>,
    pub disc_total: Option<u32>,
    pub release_date: Option<PartialDate>,
    /// Genres, multi-valued and unranked (`requirements/tags.md` §3).
    pub genres: Vec<String>,
    /// The release-type tag, authoritative for album type
    /// (`requirements/albums.md` §3).
    pub release_type: Option<String>,
    /// Explicit-content flag where tagged (`requirements/tracks.md` §6).
    pub explicit: Option<bool>,
    pub isrc: Option<String>,
    pub musicbrainz_recording_id: Option<String>,
    pub musicbrainz_release_id: Option<String>,
    /// Embedded lyrics. Sidecar lyrics are resolved separately by the scanner.
    pub lyrics: Option<Lyrics>,
    /// Whether the file carries embedded artwork (`requirements/scanning.md` §3.1).
    pub has_embedded_art: bool,
    // Sort tags (`requirements/conventions.md` §4).
    pub title_sort: Option<String>,
    pub artist_sort: Option<String>,
    pub album_sort: Option<String>,
    pub album_artist_sort: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_keep_tag_precision() {
        assert_eq!(
            PartialDate::parse("1991"),
            Some(PartialDate { year: 1991, month: None, day: None })
        );
        assert_eq!(
            PartialDate::parse("1991-09"),
            Some(PartialDate { year: 1991, month: Some(9), day: None })
        );
        assert_eq!(
            PartialDate::parse("1991-09-24T00:00:00"),
            Some(PartialDate { year: 1991, month: Some(9), day: Some(24) })
        );
        assert_eq!(PartialDate::parse("unknown"), None);
        assert_eq!(PartialDate::parse("1991-13"), None);
        assert_eq!(PartialDate::parse("2001-00"), None);
        assert_eq!(PartialDate::parse("2001-01-00"), None);
    }

    #[test]
    fn dates_format_at_their_precision() {
        assert_eq!(PartialDate::parse("1991").unwrap().to_string(), "1991");
        assert_eq!(PartialDate::parse("1991-9").unwrap().to_string(), "1991-09");
        assert_eq!(PartialDate::parse("1991-09-24").unwrap().to_string(), "1991-09-24");
    }

    #[test]
    fn any_timed_line_makes_lyrics_synced() {
        let line = |start_ms, text: &str| SyncedLine { start_ms, text: text.into() };
        assert_eq!(
            Lyrics::parse("[ti:Song]\n[00:12.50]Hello\n[Chorus]"),
            Some(Lyrics::Synced(vec![line(12_500, "Hello")]))
        );
        // A byte order mark hides no stamp, and neither does a long untimed preamble.
        let preamble = "words\n".repeat(60);
        assert_eq!(
            Lyrics::parse(&format!("\u{FEFF}{preamble}[01:02]Hello")),
            Some(Lyrics::Synced(vec![line(62_000, "Hello")]))
        );
    }

    #[test]
    fn a_byte_order_mark_keeps_the_first_line() {
        let lyrics = Lyrics::parse("\u{FEFF}[00:01.00]First\n[00:02.00]Second");
        let Some(Lyrics::Synced(lines)) = lyrics else { panic!("{lyrics:?}") };
        assert_eq!(lines[0], SyncedLine { start_ms: 1_000, text: "First".into() });
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn text_without_a_valid_stamp_is_plain() {
        for text in ["Hello [verse 1]", "[Chorus]\nla la", "[00:60.00]x", "[00:01.1234]x"] {
            assert_eq!(Lyrics::parse(text), Some(Lyrics::Plain(text.into())), "{text}");
        }
    }

    #[test]
    fn plain_text_loses_its_byte_order_mark() {
        assert_eq!(Lyrics::parse("\u{FEFF} Hello\n"), Some(Lyrics::Plain("Hello".into())));
        assert_eq!(Lyrics::parse("\u{FEFF} \n"), None);
    }
}
