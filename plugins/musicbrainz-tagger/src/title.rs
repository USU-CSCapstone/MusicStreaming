//! Reading what a title says about a track, without any I/O: the song's own title once what a
//! video or download site adds is taken out, and the artists, when it names them as
//! `Artist feat. Other - Title`. Tested natively.
//!
//! This reads titles people wrote, so it guesses. It is used to search, where a wrong guess only
//! finds nothing, and, if allowed, to fill in a track MusicBrainz has no match for.

use crate::tags::Tags;

/// What a title says.
#[derive(Debug, Default, PartialEq)]
pub struct Read {
    /// The song's title: `FRANCHISE (REMIX)` from
    /// `Travis Scott feat. Future - FRANCHISE (REMIX - Official Audio)`.
    pub title: String,
    /// The artist it is by, before any `feat.`: `Travis Scott`. Never split further, since
    /// `Simon & Garfunkel` is one artist.
    pub artist: Option<String>,
    /// The artists it features, from `feat. A, B & C` before the ` - ` or in the title's
    /// brackets: each one apart.
    pub featured: Vec<String>,
}

impl Read {
    /// Every artist it names, the main one first, or none if it names no main one.
    pub fn artists(&self) -> Vec<String> {
        let Some(artist) = &self.artist else { return Vec::new() };
        let mut artists = vec![artist.clone()];
        for f in &self.featured {
            if !artists.iter().any(|a| a.eq_ignore_ascii_case(f)) {
                artists.push(f.clone());
            }
        }
        artists
    }

    /// What to fill in for a track MusicBrainz has no match for: the song's title, replacing
    /// the one that named the artist, and the artists. None when it names no artist, as there is
    /// nothing worth writing then.
    pub fn guess(&self) -> Option<Tags> {
        let artists = self.artists();
        if artists.is_empty() || self.title.is_empty() {
            return None;
        }
        Some(Tags {
            title: self.title.clone(),
            retitle: true,
            artists,
            album_artists: Vec::new(),
            album: None,
            track: None,
            disc: None,
            date: None,
        })
    }

    /// The main artist cut to its first name, for searching: `Simon` from `Simon & Garfunkel`
    /// still finds them, since a search matches any credit with that name in it.
    pub fn search_artist(&self) -> Option<&str> {
        let artist = self.artist.as_deref()?;
        let cut = [",", " & ", " x ", " X ", " and ", " with "]
            .iter()
            .filter_map(|sep| artist.find(sep))
            .min()
            .unwrap_or(artist.len());
        Some(artist[..cut].trim()).filter(|a| !a.is_empty())
    }
}

/// `title` with a leading track number and what a video site adds taken out, but its artist,
/// if it names one, left in.
pub fn clean(title: &str) -> String {
    without_groups(without_number(title.trim()), &mut Vec::new())
}

/// What `title` says: see [`Read`]. A leading track number of one to three digits, as a file
/// name has (`01 - `, `01. `, `01_`), is dropped first.
pub fn read(title: &str) -> Read {
    let title = without_number(title.trim());
    let mut featured = Vec::new();
    let title = without_groups(title, &mut featured);
    let mut read = match title.split_once(" - ") {
        Some((artist, rest)) if !artist.trim().is_empty() && !rest.trim().is_empty() => {
            let (artist, feat) = split_feat(artist.trim());
            if let Some(feat) = feat {
                featured.splice(0..0, names(feat));
            }
            Read { title: tidy(rest), artist: Some(artist.into()), featured: Vec::new() }
        }
        _ => Read { title: tidy(&title), ..Read::default() },
    };
    read.featured = featured;
    read
}

/// `title` without a leading track number of one to three digits followed by `. `, ` - `, or
/// `_`. A title that is only a number, such as `1979`, is left alone.
fn without_number(title: &str) -> &str {
    let digits = title.bytes().take_while(u8::is_ascii_digit).count();
    if !(1..=3).contains(&digits) {
        return title;
    }
    let rest = &title[digits..];
    [". ", " - ", "_"]
        .iter()
        .find_map(|sep| rest.strip_prefix(sep))
        .map(str::trim)
        .filter(|rest| !rest.is_empty())
        .unwrap_or(title)
}

/// `title` with its bracketed parts, `(…)` and `[…]`, that are noise taken out, the ones that
/// end in noise cut short (`(REMIX - Official Audio)` to `(REMIX)`), and the `(feat. …)` ones
/// taken out with their artists added to `featured`. A trailing ` - Official Audio` and the
/// like goes too.
fn without_groups(title: &str, featured: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(title.len());
    let mut rest = title;
    while let Some(open) = rest.find(['(', '[']) {
        let close = if rest[open..].starts_with('(') { ')' } else { ']' };
        let Some(len) = rest[open + 1..].find(close) else { break };
        out.push_str(&rest[..open]);
        let inside = rest[open + 1..open + 1 + len].trim();
        if let (_, Some(feat)) = split_feat(&format!(" {inside}")) {
            featured.extend(names(feat));
        } else if !is_noise(inside) {
            let kept = match inside.rsplit_once(" - ") {
                Some((before, after)) if is_noise(after) => before.trim(),
                _ => inside,
            };
            let (open, close) = if close == ')' { ('(', ')') } else { ('[', ']') };
            out.push_str(&format!("{open}{kept}{close}"));
        }
        rest = &rest[open + 1 + len + 1..];
    }
    out.push_str(rest);
    let out = tidy(&out);
    match out.rsplit_once(" - ") {
        Some((before, after)) if is_noise(after) => before.trim().to_owned(),
        _ => out,
    }
}

/// Whether `s` is only what a video or download site adds to a title: `Official Audio`,
/// `Lyric Video`, `HD`, and the like. Every word must be one of those.
fn is_noise(s: &str) -> bool {
    const WORDS: &[&str] = &[
        "official",
        "audio",
        "video",
        "music",
        "lyric",
        "lyrics",
        "visualizer",
        "visualiser",
        "hd",
        "hq",
        "4k",
        "explicit",
        "clean",
        "full",
        "high",
        "quality",
        "mv",
        "m/v",
    ];
    let words: Vec<String> = s
        .split(|c: char| c.is_whitespace() || c == '-' || c == '|')
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    !words.is_empty() && words.iter().all(|w| WORDS.contains(&w.as_str()))
}

/// `artist` split at its `feat.`, `ft.`, or `featuring`, if it has one, into who it is by and
/// who it features.
fn split_feat(artist: &str) -> (&str, Option<&str>) {
    let lower = artist.to_lowercase();
    let at = [" feat. ", " feat ", " ft. ", " ft ", " featuring "]
        .iter()
        .filter_map(|marker| lower.find(marker).map(|i| (i, marker.len())))
        .min();
    // Lowercasing can change byte lengths outside ASCII, so only cut where both agree.
    match at {
        Some((i, len)) if lower.len() == artist.len() => {
            (artist[..i].trim(), Some(artist[i + len..].trim()).filter(|f| !f.is_empty()))
        }
        _ => (artist, None),
    }
}

/// The names in a list of featured artists, `A, B & C` or `A and B`.
fn names(list: &str) -> Vec<String> {
    list.split(", ")
        .flat_map(|part| part.split(" & "))
        .flat_map(|part| part.split(" and "))
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .collect()
}

/// `s` with its runs of whitespace made single spaces, and none at either end.
fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
