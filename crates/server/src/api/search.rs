//! Search: `search` (`api/openapi.yaml`), over the core's index (`jewelcase_core::Index`),
//! which each library keeps in memory (`indexes`).

mod indexes;
mod tagged;

use std::collections::HashSet;
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
use super::tag_filter::TagFilter;
use super::tracks::{self, TrackSummary};
use super::{Problem, hidden, page};
use crate::db::{Database, by_ids};

/// The spec's `search` parameters.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchQuery {
    q: String,
    /// Comma-separated section types.
    types: Option<String>,
    tag_all: Option<String>,
    tag_any: Option<String>,
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

/// The spec's `search`: each section's best matches for `q` in `library`, of those carrying the
/// tags `tagAll` and `tagAny` name.
pub async fn search(
    State(db): State<Arc<Database>>,
    State(indexes): State<Arc<Indexes>>,
    Scope { user, library }: Scope,
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
    let filter = TagFilter::new(query.tag_all.as_deref(), query.tag_any.as_deref())?;
    let tagged = indexes.tagged.get(&db, library, &filter, &kinds).await?;
    let index = indexes.index(&db, library).await?;
    // What the caller hid is gone from their library, so a search finds it nowhere: tracks, and
    // albums and artists every track of which they hid.
    let hid: [HashSet<i64>; 3] = db
        .read(move |conn| {
            // By `Kind`, as the tagged sets are.
            Ok::<_, rusqlite::Error>([
                hidden::ids(conn, user, library)?,
                hidden::albums(conn, user, library)?.into_iter().collect(),
                hidden::artists(conn, user, library)?.into_iter().collect(),
            ])
        })
        .await?;
    let found = index.search_within(&query.q, &kinds, limit, |kind, id| {
        !hid[kind as usize].contains(&id)
            && tagged.as_ref().is_none_or(|tagged| tagged[kind as usize].contains(&id))
    });
    let sections = db.read(move |conn| results(conn, user, library, found)).await?;

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

/// The summaries of what the index found, as the browse lists show them to `user`.
fn results(
    conn: &Connection,
    user: i64,
    library: i64,
    found: Vec<Found>,
) -> rusqlite::Result<Vec<Section>> {
    let mut sections = Vec::with_capacity(found.len());
    let losses = hidden::losses(conn, user, library)?;
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
                album.lose(&losses);
                Ok(SearchResult::Album { album })
            })?,
            Kind::Artist => by_ids(conn, artists::SELECT, "artists ar", "", library, ids, |row| {
                let mut artist = artists::summary(row)?;
                artist.lose(&losses);
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
