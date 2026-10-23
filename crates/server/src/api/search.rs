//! Search: `search` (`api/openapi.yaml`), over the core's index (`jewelcase_core::Index`),
//! which each library keeps in memory (`indexes`).

mod indexes;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_core::{Kind, Section as Found};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub use indexes::Indexes;

use super::albums::{self, AlbumSummary};
use super::artists::{self, ArtistSummary};
use super::extract::Query;
use super::scope::Scope;
use super::tracks::{self, TrackSummary};
use super::{Problem, page};
use crate::db::{Database, by_ids};

/// The spec's `search` parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchQuery {
    q: String,
    /// Comma-separated section types.
    types: Option<String>,
    section_limit: Option<usize>,
}

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

/// The spec's `search`: each section's best matches for `q` in `library`.
pub async fn search(
    State(db): State<Arc<Database>>,
    State(indexes): State<Arc<Indexes>>,
    Scope { library, .. }: Scope,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, Problem> {
    let invalid = |detail| Problem::invalid(detail);
    if query.q.is_empty() {
        return Err(invalid("q must not be empty"));
    }
    let limit = query.section_limit.unwrap_or(5);
    if !(1..=50).contains(&limit) {
        return Err(invalid("sectionLimit must be from 1 to 50"));
    }
    let kinds = query.types.as_deref().map_or_else(|| Ok(Kind::ALL.to_vec()), kinds)?;

    let track_count =
        db.read(move |conn| page::library_number(conn, library, "track_count")).await?;
    let index = indexes.index(&db, library).await?;
    let found = index.search(&query.q, &kinds, limit);
    let sections = db.read(move |conn| results(conn, library, found)).await?;

    Ok(Json(SearchResponse {
        query: query.q,
        top: sections.first().map(|section| section.items[0].clone()),
        sections,
        near_misses: Vec::new(),
        library_empty: track_count == 0,
    }))
}

/// The kinds to search for the comma-separated section `types`. Playlists add none, since there
/// are none yet and an empty section is the true answer. Lyrics answer `422` until they are
/// indexed, as a list answers for a filter it does not support yet.
fn kinds(types: &str) -> Result<Vec<Kind>, Problem> {
    let mut kinds = Vec::new();
    for section in types.split(',') {
        match section {
            "playlists" => {}
            "lyrics" => return Err(Problem::invalid("lyrics cannot be searched yet")),
            _ => {
                let Some(kind) = Kind::ALL.into_iter().find(|&kind| section_type(kind) == section)
                else {
                    return Err(Problem::invalid(
                        "types must be tracks, albums, artists, playlists, or lyrics",
                    ));
                };
                kinds.push(kind);
            }
        }
    }
    Ok(kinds)
}

/// How many of `library`'s tracks, albums, and artists match `q`, as a search counts them, in
/// the order of `Kind::ALL`.
pub async fn totals(
    db: &Arc<Database>,
    indexes: &Indexes,
    library: i64,
    q: &str,
) -> Result<[usize; 3], Problem> {
    let index = indexes.index(db, library).await?;
    let found = index.search(q, &Kind::ALL, 1);
    Ok(Kind::ALL.map(|kind| {
        found.iter().find(|section| section.kind == kind).map_or(0, |section| section.total)
    }))
}

/// The spec's name for the section of `kind`.
fn section_type(kind: Kind) -> &'static str {
    match kind {
        Kind::Track => "tracks",
        Kind::Album => "albums",
        Kind::Artist => "artists",
    }
}

/// The summaries of what the index found, as the browse lists show them.
fn results(conn: &Connection, library: i64, found: Vec<Found>) -> rusqlite::Result<Vec<Section>> {
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

#[cfg(test)]
mod tests;
