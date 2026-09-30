//! Search: `search` (`api/openapi.yaml`), over the core's index (`jewelcase_core::search`),
//! which each library keeps in memory (`indexes`).

mod indexes;

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use jewelcase_core::search::{Kind, Section as Found};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub use indexes::Indexes;

use super::albums::{self, AlbumSummary};
use super::artists::{self, ArtistSummary};
use super::extract::{Path, Query};
use super::tracks::{self, TrackSummary};
use super::{Id, Problem, page, sql};
use crate::db::Database;

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

pub async fn search(
    State(db): State<Arc<Database>>,
    State(indexes): State<Arc<Indexes>>,
    Path(Id(library)): Path<Id>,
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
    let mut kinds = Vec::new();
    for section in query
        .types
        .as_deref()
        .map_or(vec!["tracks", "albums", "artists"], |t| {
            t.split(',').collect()
        })
    {
        match section {
            "tracks" => kinds.push(Kind::Track),
            "albums" => kinds.push(Kind::Album),
            "artists" => kinds.push(Kind::Artist),
            // There are no playlists yet, and lyrics are not indexed yet.
            "playlists" | "lyrics" => {}
            _ => {
                return Err(invalid(
                    "types must be tracks, albums, artists, playlists, or lyrics",
                ));
            }
        }
    }

    let (track_count, position) = db
        .read(move |conn| {
            let Some(track_count) = page::library_count(conn, library, "track_count")? else {
                return Ok(None);
            };
            Ok(Some((track_count, indexes::feed_position(conn, library)?)))
        })
        .await?
        .ok_or_else(Problem::not_found)?;
    let index = indexes.index(&db, library, position).await?;
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

/// The summaries of what the index found, as the browse lists show them.
fn results(conn: &Connection, library: i64, found: Vec<Found>) -> rusqlite::Result<Vec<Section>> {
    // Tracks of one album share its reference, so each album is read once.
    let mut album_refs = HashMap::new();
    let mut sections = Vec::with_capacity(found.len());
    for section in found {
        let ids = &section.ids;
        let items = match section.kind {
            Kind::Track => sql::by_ids(conn, tracks::SELECT, "tracks t", library, ids, |row| {
                let track = tracks::list_summary(conn, row, &mut album_refs)?;
                Ok(SearchResult::Track { track })
            })?,
            Kind::Album => sql::by_ids(conn, albums::SELECT, "albums al", library, ids, |row| {
                let album = albums::summary(conn, row)?;
                Ok(SearchResult::Album { album })
            })?,
            Kind::Artist => {
                sql::by_ids(conn, artists::SELECT, "artists ar", library, ids, |row| {
                    let artist = artists::summary(row)?;
                    Ok(SearchResult::Artist { artist })
                })?
            }
        };
        // Gone since the index was built: nothing of it is left to show.
        if !items.is_empty() {
            let kind = match section.kind {
                Kind::Track => "tracks",
                Kind::Album => "albums",
                Kind::Artist => "artists",
            };
            sections.push(Section {
                kind,
                items,
                total: section.total,
            });
        }
    }
    Ok(sections)
}

#[cfg(test)]
mod tests;
