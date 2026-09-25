//! The canonical tag set (`requirements/scanning.md` §2, `design/scanning.md` §4).
//!
//! Whatever the tagging scheme, the scanner produces exactly this. Two files
//! carrying the same logical metadata in ID3v2 and Vorbis comments yield
//! equal `TagSet`s; that equality is what the golden-corpus test asserts.

use serde::{Deserialize, Serialize};

/// A date at whatever precision the tag carried (`requirements/tracks.md` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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
        let year: u16 = parts
            .next()?
            .parse()
            .ok()
            .filter(|y| (1000..=9999).contains(y))?;
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

/// Lyrics as read from the file or a sidecar (`requirements/tracks.md` §5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub text: String,
    /// True when the text carries LRC-style timestamps.
    pub synced: bool,
}

impl Lyrics {
    /// Wrap text, detecting synchronization from `[mm:ss.xx]` markers.
    pub fn from_text(text: String) -> Lyrics {
        let synced = text.lines().take(50).any(is_lrc_line);
        Lyrics { text, synced }
    }
}

fn is_lrc_line(line: &str) -> bool {
    let line = line.trim_start();
    let Some(rest) = line.strip_prefix('[') else {
        return false;
    };
    let Some(end) = rest.find(']') else {
        return false;
    };
    let stamp = &rest[..end];
    let mut it = stamp.split(':');
    match (it.next(), it.next()) {
        (Some(m), Some(s)) => {
            m.chars().all(|c| c.is_ascii_digit())
                && !m.is_empty()
                && s.split('.')
                    .next()
                    .is_some_and(|sec| sec.len() == 2 && sec.chars().all(|c| c.is_ascii_digit()))
        }
        _ => false,
    }
}

/// Everything Jewelcase reads from a file's tags. Every field is optional or
/// empty by default; an untagged file is a valid `TagSet`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
            Some(PartialDate {
                year: 1991,
                month: None,
                day: None
            })
        );
        assert_eq!(
            PartialDate::parse("1991-09"),
            Some(PartialDate {
                year: 1991,
                month: Some(9),
                day: None
            })
        );
        assert_eq!(
            PartialDate::parse("1991-09-24T00:00:00"),
            Some(PartialDate {
                year: 1991,
                month: Some(9),
                day: Some(24)
            })
        );
        assert_eq!(PartialDate::parse("unknown"), None);
        assert_eq!(PartialDate::parse("1991-13"), None);
    }

    #[test]
    fn dates_format_at_their_precision() {
        assert_eq!(PartialDate::parse("1991").unwrap().to_string(), "1991");
        assert_eq!(PartialDate::parse("1991-9").unwrap().to_string(), "1991-09");
        assert_eq!(
            PartialDate::parse("1991-09-24").unwrap().to_string(),
            "1991-09-24"
        );
    }

    #[test]
    fn lrc_is_detected() {
        assert!(Lyrics::from_text("[00:12.50]Hello".into()).synced);
        assert!(Lyrics::from_text("[01:02]Hello".into()).synced);
        assert!(!Lyrics::from_text("Hello [verse 1]".into()).synced);
        assert!(!Lyrics::from_text("[Chorus]\nla la".into()).synced);
    }
}
