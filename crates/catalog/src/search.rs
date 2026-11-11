//! Search: the documents a library's search index is built from, and the response a search
//! gives (`search`, `api/openapi.yaml`). The index itself is the core's
//! (`jewelcase_core::Index`), which the server keeps in memory and a device builds from the same
//! documents.

use std::collections::HashSet;

use jewelcase_core::{Document, Kind, Section as Found};
use rusqlite::Connection;
use serde::Serialize;

use crate::Invalid;
use crate::albums::{self, AlbumSummary};
use crate::artists::{self, ArtistSummary};
use crate::by_ids::by_ids;
use crate::hidden::{self, Losses};
use crate::page::library_number;
use crate::tag_filter::{TagFilter, Tagged, Tags};
use crate::tracks::{self, TrackSummary};

/// The spec's `SearchResponse`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    query: String,
    top: Option<SearchResult>,
    sections: Vec<Section>,
    near_misses: Vec<String>,
    library_empty: bool,
}

/// One of a `SearchResponse`'s `sections`.
#[derive(Serialize)]
struct Section {
    #[serde(rename = "type")]
    kind: &'static str,
    items: Vec<SearchResult>,
    total: usize,
}

/// The spec's `SearchResult`: `{"type": "track", "track": {…}}` and so on.
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum SearchResult {
    Track { track: TrackSummary },
    Album { album: AlbumSummary },
    Artist { artist: ArtistSummary },
}

/// The library's tracks, albums and artists, as the core's rule makes them documents.
pub fn documents(conn: &Connection, library: i64) -> rusqlite::Result<Vec<Document>> {
    let mut documents = Vec::new();
    let mut tracks = conn.prepare_cached("SELECT id, title FROM tracks WHERE library_id = ?1")?;
    for track in
        tracks.query_map([library], |row| Ok(Document::from_track(row.get(0)?, row.get(1)?)))?
    {
        documents.push(track?);
    }
    let mut albums = conn.prepare_cached("SELECT id, title FROM albums WHERE library_id = ?1")?;
    for album in
        albums.query_map([library], |row| Ok(Document::from_album(row.get(0)?, row.get(1)?)))?
    {
        documents.extend(album?);
    }
    let mut artists = conn.prepare_cached("SELECT id, name FROM artists WHERE library_id = ?1")?;
    for artist in
        artists.query_map([library], |row| Ok(Document::from_artist(row.get(0)?, row.get(1)?)))?
    {
        documents.extend(artist?);
    }
    Ok(documents)
}

/// How many matches a search for `q` returns in each section, from its `sectionLimit`: 5 unless
/// it names 1 to 50. [`Invalid`] for an empty `q`.
pub fn section_limit(q: &str, section_limit: Option<usize>) -> Result<usize, Invalid> {
    if q.is_empty() {
        return Err(Invalid("q must not be empty".to_owned()));
    }
    let limit = section_limit.unwrap_or(5);
    if !(1..=50).contains(&limit) {
        return Err(Invalid("sectionLimit must be from 1 to 50".to_owned()));
    }
    Ok(limit)
}

/// The kinds to search for the comma-separated section `types`. Playlists add none, since the
/// index holds no playlists and an empty section is the true answer. Lyrics are [`Invalid`] until
/// they are indexed, as a list is for a filter it does not support yet.
pub fn kinds(types: &str) -> Result<Vec<Kind>, Invalid> {
    let mut kinds = Vec::new();
    for section in types.split(',') {
        match section {
            "playlists" => {}
            "lyrics" => return Err(Invalid("lyrics cannot be searched yet".to_owned())),
            _ => {
                let Some(kind) = Kind::ALL.into_iter().find(|&kind| section_type(kind) == section)
                else {
                    return Err(Invalid(
                        "types must be tracks, albums, artists, playlists, or lyrics".to_owned(),
                    ));
                };
                kinds.push(kind);
            }
        }
    }
    Ok(kinds)
}

/// For each kind in `Kind::ALL` order, the IDs of the rows of `library` of it that carry the
/// tags `filter` names, as each list's own filter links them ([`Tags::ids`]): none for a kind not
/// in `kinds`. The caller asks only for a filter that names tags; for one that names none every
/// set is empty.
pub fn tagged(
    conn: &Connection,
    library: i64,
    filter: &TagFilter,
    kinds: &[Kind],
) -> rusqlite::Result<[HashSet<i64>; 3]> {
    let mut sets: [HashSet<i64>; 3] = Default::default();
    for &kind in kinds {
        let tagged: &Tagged = match kind {
            Kind::Track => &tracks::TAGGED,
            Kind::Album => &albums::TAGGED,
            Kind::Artist => &artists::TAGGED,
        };
        let ids = Tags::read(conn, library, tagged, filter)?.ids(conn, tagged)?;
        sets[kind as usize] = ids.unwrap_or_default();
    }
    Ok(sets)
}

/// What `user` hid of `library`, as a search leaves it out: by [`Kind`], the tracks they hid and
/// the albums and artists every track of which they hid, with the albums and artists the tags
/// `filter` names reach only through hidden tracks; and what the hidden tracks take from the
/// counts of the albums and artists found.
pub struct Hid {
    sets: [HashSet<i64>; 3],
    losses: Losses,
}

impl Hid {
    /// Reads what `user` hid in `library`, for a search filtered by `filter`.
    pub fn read(
        conn: &Connection,
        user: i64,
        library: i64,
        filter: &TagFilter,
    ) -> rusqlite::Result<Hid> {
        let losses = hidden::losses(conn, user, library)?;
        // By `Kind`, as the tagged sets are.
        let mut sets: [HashSet<i64>; 3] = [
            hidden::ids(conn, user, library)?,
            losses.whole_albums.iter().copied().collect(),
            losses.whole_artists.iter().copied().collect(),
        ];
        if !filter.is_empty() && !sets[Kind::Track as usize].is_empty() {
            let albums = losses.albums.iter().map(|(&album, loss)| (album, &loss.tags));
            let tagged = &albums::TAGGED;
            let lost = Tags::read(conn, library, tagged, filter)?.lost(conn, tagged, albums)?;
            sets[Kind::Album as usize].extend(lost);
            let artists = losses.artists.iter().map(|(&artist, loss)| (artist, &loss.tags));
            let tagged = &artists::TAGGED;
            let lost = Tags::read(conn, library, tagged, filter)?.lost(conn, tagged, artists)?;
            sets[Kind::Artist as usize].extend(lost);
        }
        Ok(Hid { sets, losses })
    }

    /// Whether the row `id` of `kind` is hidden, so a search leaves it out.
    pub fn contains(&self, kind: Kind, id: i64) -> bool {
        self.sets[kind as usize].contains(&id)
    }
}

/// The spec's name for the section of `kind`.
fn section_type(kind: Kind) -> &'static str {
    match kind {
        Kind::Track => "tracks",
        Kind::Album => "albums",
        Kind::Artist => "artists",
    }
}

/// The response to a search for `q` in `library` that found `found` for `user`, whose hidden
/// tracks `hid` says: the summaries of what the index found, as the browse lists show them to
/// `user`, and whether the library holds no track at all.
pub fn response(
    conn: &Connection,
    user: i64,
    library: i64,
    q: String,
    found: Vec<Found>,
    hid: &Hid,
) -> rusqlite::Result<SearchResponse> {
    let sections = sections(conn, user, library, found, &hid.losses)?;
    Ok(SearchResponse {
        query: q,
        top: sections.first().map(|section| section.items[0].clone()),
        sections,
        near_misses: Vec::new(),
        library_empty: library_number(conn, library, "track_count")? == 0,
    })
}

/// The summaries of what the index found, a section for each kind that has any left to show, as
/// the browse lists show them to `user`, whose hidden tracks take `losses` from them.
fn sections(
    conn: &Connection,
    user: i64,
    library: i64,
    found: Vec<Found>,
    losses: &Losses,
) -> rusqlite::Result<Vec<Section>> {
    let mut sections = Vec::with_capacity(found.len());
    for section in found {
        let ids = &section.ids;
        let items = match section.kind {
            Kind::Track => {
                let mut found =
                    by_ids(conn, tracks::SELECT, "tracks t", tracks::JOINS, library, ids, |row| {
                        tracks::list_summary(row)
                    })?;
                tracks::mark_liked(conn, user, library, &mut found)?;
                found.into_iter().map(|track| SearchResult::Track { track }).collect()
            }
            Kind::Album => by_ids(conn, albums::SELECT, "albums al", "", library, ids, |row| {
                let mut album = albums::summary(row)?;
                album.lose(losses);
                Ok(SearchResult::Album { album })
            })?,
            Kind::Artist => by_ids(conn, artists::SELECT, "artists ar", "", library, ids, |row| {
                let mut artist = artists::summary(row)?;
                artist.lose(losses);
                Ok(SearchResult::Artist { artist })
            })?,
        };
        // Gone since the index was built: nothing of it is left to show.
        if !items.is_empty() {
            let kind = section_type(section.kind);
            sections.push(Section { kind, items, total: section.total });
        }
    }
    Ok(sections)
}
