//! Search: the documents a library's search index is built from, and the response a search
//! gives (`search`, `api/openapi.yaml`). The index itself is the core's
//! (`jewelcase_core::Index`), which the server keeps in memory and a device builds from the same
//! documents.

use jewelcase_core::{Document, Kind, Section as Found};
use rusqlite::Connection;
use serde::Serialize;

use crate::Invalid;
use crate::albums::{self, AlbumSummary};
use crate::artists::{self, ArtistSummary};
use crate::by_ids::by_ids;
use crate::page::library_number;
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

/// The kinds to search for the comma-separated section `types`. Playlists add none, since the
/// index holds no playlists and an empty section is the true answer. Lyrics are [`Invalid`] until they are
/// indexed, as a list is for a filter it does not support yet.
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

/// The spec's name for the section of `kind`.
fn section_type(kind: Kind) -> &'static str {
    match kind {
        Kind::Track => "tracks",
        Kind::Album => "albums",
        Kind::Artist => "artists",
    }
}

/// The response to a search for `q` in `library` that found `found`: the summaries of what the
/// index found, as the browse lists show them, and whether the library holds no track at all.
pub fn response(
    conn: &Connection,
    library: i64,
    q: String,
    found: Vec<Found>,
) -> rusqlite::Result<SearchResponse> {
    let sections = sections(conn, library, found)?;
    Ok(SearchResponse {
        query: q,
        top: sections.first().map(|section| section.items[0].clone()),
        sections,
        near_misses: Vec::new(),
        library_empty: library_number(conn, library, "track_count")? == 0,
    })
}

/// The summaries of what the index found, a section for each kind that has any left to show.
fn sections(conn: &Connection, library: i64, found: Vec<Found>) -> rusqlite::Result<Vec<Section>> {
    let mut sections = Vec::with_capacity(found.len());
    for section in found {
        let ids = &section.ids;
        let items = match section.kind {
            Kind::Track => {
                by_ids(conn, tracks::SELECT, "tracks t", tracks::JOINS, library, ids, |row| {
                    let track = tracks::list_summary(row)?;
                    Ok(SearchResult::Track { track })
                })?
            }
            Kind::Album => by_ids(conn, albums::SELECT, "albums al", "", library, ids, |row| {
                let album = albums::summary(row)?;
                Ok(SearchResult::Album { album })
            })?,
            Kind::Artist => by_ids(conn, artists::SELECT, "artists ar", "", library, ids, |row| {
                let artist = artists::summary(row)?;
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
