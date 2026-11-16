//! Asking MusicBrainz (https://musicbrainz.org/doc/MusicBrainz_API/Search) which recording a track
//! with no artist is, without any I/O: what to search for, the request URL, reading the answer,
//! and choosing what to write from it. Tested natively.

use serde::Deserialize;

use crate::failure::Failure;
use crate::tags::Tags;
use crate::title;

pub const API: &str = "https://musicbrainz.org/ws/2";

/// What every request names as its client. MusicBrainz asks for a name, a version, and a way to
/// reach whoever runs it, and may refuse a client that sends none.
pub const USER_AGENT: &str = concat!(
    "JewelcaseMusicBrainzTagger/",
    env!("CARGO_PKG_VERSION"),
    " ( https://github.com/USU-CSCapstone/MusicStreaming )"
);

/// How far a recording's length may be from the track's and still be the same recording, when
/// only its title is known.
pub const LENGTH_TOLERANCE_MS: u64 = 3_000;

/// How far it may be when the artist is known too. A song saved from a video often runs a few
/// seconds longer or shorter than the release, and the artist already narrows the search.
pub const ARTIST_LENGTH_TOLERANCE_MS: u64 = 15_000;

/// The most recordings one search asks for. Every one within the tolerance counts towards
/// whether the match is ambiguous, so this is generous.
const LIMIT: u32 = 50;

/// The size of cover asked of the Cover Art Archive, in pixels across: large enough for the
/// player, small enough to keep the file small.
const COVER_SIZE: u32 = 500;

/// One search for a track.
#[derive(Debug, PartialEq)]
pub struct Query {
    /// The song's title, with what a video site adds taken out.
    pub title: String,
    /// The artist, when the title named one as `Artist - Title`.
    pub artist: Option<String>,
    pub length_ms: u64,
    /// Whether the track's title differs from the song's, so a match replaces it.
    pub retitle: bool,
}

impl Query {
    /// How far a recording's length may be from the track's: further when the artist is known.
    pub fn tolerance_ms(&self) -> u64 {
        if self.artist.is_some() { ARTIST_LENGTH_TOLERANCE_MS } else { LENGTH_TOLERANCE_MS }
    }
}

/// The searches to try for a track titled `title`, in order, until one finds something
/// (`title.rs` reads it). A title that names its artist, `Artist feat. Other - Title (Official
/// Audio)`, is searched as the song `Title` by `Artist` first, then by its whole title, since
/// some titles have ` - ` in them.
pub fn queries(title: &str, length_ms: u64) -> Vec<Query> {
    let read = title::read(title);
    let whole = title::clean(title);
    let retitle = |t: &str| t != title.trim();
    let mut queries = Vec::new();
    if let Some(artist) = read.search_artist() {
        let artist = Some(artist.to_owned());
        let retitle = retitle(&read.title);
        queries.push(Query { title: read.title.clone(), artist, length_ms, retitle });
    }
    if queries.iter().all(|q| q.title != whole) {
        queries.push(Query { retitle: retitle(&whole), title: whole, artist: None, length_ms });
    }
    queries
}

/// Percent-encodes `s` for a query string.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `s` as a quoted Lucene phrase, in which only `"` and `\` need escaping.
fn phrase(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The search for `q`: recordings with its title, and artist if it has one, whose length is
/// within the tolerance of the track's.
pub fn search_url(q: &Query) -> String {
    let low = q.length_ms.saturating_sub(q.tolerance_ms());
    let high = q.length_ms.saturating_add(q.tolerance_ms());
    let mut query = format!("recording:{} AND dur:[{low} TO {high}]", phrase(&q.title));
    if let Some(artist) = &q.artist {
        query.push_str(&format!(" AND artist:{}", phrase(artist)));
    }
    format!("{API}/recording?query={}&limit={LIMIT}&fmt=json", encode(&query))
}

#[derive(Debug, Deserialize)]
struct Search {
    #[serde(default)]
    recordings: Vec<Recording>,
}

/// One recording a search found; the fields the plugin uses.
#[derive(Debug, Deserialize)]
pub struct Recording {
    title: String,
    #[serde(default)]
    length: Option<u64>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<Credit>,
    #[serde(default)]
    releases: Vec<Release>,
}

/// One artist named in a credit.
#[derive(Debug, Deserialize)]
struct Credit {
    name: String,
}

/// A release a recording is on.
#[derive(Debug, Deserialize)]
struct Release {
    id: String,
    title: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    date: Option<String>,
    /// Left out when it is the recording's own.
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<Credit>,
    #[serde(rename = "release-group", default)]
    release_group: Option<ReleaseGroup>,
    /// The medium the recording is on, holding only its track.
    #[serde(default)]
    media: Vec<Medium>,
}

#[derive(Debug, Deserialize)]
struct ReleaseGroup {
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "primary-type", default)]
    primary_type: Option<String>,
    #[serde(rename = "secondary-types", default)]
    secondary_types: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Medium {
    #[serde(default)]
    position: Option<u32>,
    #[serde(default, alias = "tracks")]
    track: Vec<MediumTrack>,
}

#[derive(Debug, Deserialize)]
struct MediumTrack {
    number: String,
}

/// What a search came to.
#[derive(Debug, PartialEq)]
pub enum Found {
    /// One artist's recording matched.
    Match(Match),
    /// Recordings by more than one artist matched, named here, so nothing is written.
    Ambiguous(Vec<String>),
    /// Nothing matched.
    Nothing,
}

/// Whether MusicBrainz answered `status` because it was asked too often: a `503`, its usual
/// answer, or a `429`. Asking again after a pause may work.
pub fn turned_away(status: u16) -> bool {
    matches!(status, 429 | 503)
}

/// A recording a search matched.
#[derive(Debug, PartialEq)]
pub struct Match {
    /// The tags to fill in.
    pub tags: Tags,
    /// Where its release's front cover may be, best first: the release's own, then any of its
    /// release group's. Empty when it is on no release.
    pub covers: Vec<String>,
}

/// The recordings in MusicBrainz's answer to a search.
pub fn parse(status: u16, body: &str) -> Result<Vec<Recording>, Failure> {
    if status != 200 {
        return Err(Failure::answered(status));
    }
    serde_json::from_str::<Search>(body)
        .map(|s| s.recordings)
        .map_err(|e| Failure::Failed(format!("MusicBrainz's answer could not be read: {e}")))
}

/// What to write for the track `q` searched for, from the `recordings` found. Only a recording
/// with the same title, ignoring case and punctuation, and a length within the query's tolerance
/// counts. When those are all one artist's, the closest in length gives the title and artists,
/// and their best release the album; when they are more than one artist's, the title and length
/// alone cannot tell which it is, and nothing is chosen.
pub fn choose(q: &Query, recordings: &[Recording]) -> Found {
    let title = simplified(&q.title);
    let mut matches: Vec<&Recording> = recordings
        .iter()
        .filter(|r| !r.artist_credit.is_empty() && simplified(&r.title) == title)
        .filter(|r| r.length.is_some_and(|l| l.abs_diff(q.length_ms) <= q.tolerance_ms()))
        .collect();
    matches.sort_by_key(|r| r.length.map(|l| l.abs_diff(q.length_ms)));
    let Some(first) = matches.first() else { return Found::Nothing };
    let mut artists: Vec<String> = Vec::new();
    for r in &matches {
        let credit = names(&r.artist_credit).join("; ");
        if !artists.iter().any(|a| simplified(a) == simplified(&credit)) {
            artists.push(credit);
        }
    }
    if artists.len() > 1 {
        return Found::Ambiguous(artists);
    }
    let release = matches.iter().flat_map(|r| &r.releases).min_by(|a, b| rank(a).cmp(&rank(b)));
    let artists = names(&first.artist_credit);
    let medium = release.and_then(|r| r.media.first());
    let tags = Tags {
        title: first.title.clone(),
        retitle: q.retitle,
        album_artists: release
            .map(|r| names(&r.artist_credit))
            .filter(|names| !names.is_empty())
            .unwrap_or_else(|| artists.clone()),
        artists,
        album: release.map(|r| r.title.clone()),
        track: medium.and_then(|m| m.track.first()).and_then(|t| t.number.parse().ok()),
        disc: medium.and_then(|m| m.position),
        date: release.and_then(|r| r.date.clone()).filter(|d| !d.is_empty()),
    };
    Found::Match(Match { tags, covers: release.map(covers).unwrap_or_default() })
}

/// Where `release`'s front cover may be on the Cover Art Archive, best first.
fn covers(release: &Release) -> Vec<String> {
    let base = "https://coverartarchive.org";
    let mut urls = vec![format!("{base}/release/{}/front-{COVER_SIZE}", encode(&release.id))];
    if let Some(group) = release.release_group.as_ref().and_then(|g| g.id.as_deref()) {
        urls.push(format!("{base}/release-group/{}/front-{COVER_SIZE}", encode(group)));
    }
    urls
}

/// How a release sorts when choosing the album, best first: an official one, an album over an
/// EP over a single over anything else, one that is not a compilation, live, or other secondary
/// kind, and then the earliest, so a later reissue loses to the original.
fn rank(r: &Release) -> (bool, bool, u8, bool, Option<&str>) {
    let group = r.release_group.as_ref();
    let kind = match group.and_then(|g| g.primary_type.as_deref()) {
        Some("Album") => 0,
        Some("EP") => 1,
        Some("Single") => 2,
        _ => 3,
    };
    let date = r.date.as_deref().filter(|d| !d.is_empty());
    (
        r.status.as_deref() != Some("Official"),
        group.is_some_and(|g| !g.secondary_types.is_empty()),
        kind,
        date.is_none(),
        date,
    )
}

/// The names in a credit, in order.
fn names(credit: &[Credit]) -> Vec<String> {
    credit.iter().map(|c| c.name.clone()).collect()
}

/// `s` lowercased with only its letters and digits, so `Don't Stop` and `Dont stop!` are the
/// same title.
fn simplified(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests;
