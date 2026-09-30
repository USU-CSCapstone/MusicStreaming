//! Search: `search` (`api/openapi.yaml`), over the core's index (`jewelcase_core::search`).
//!
//! Each library's index lives in memory, tagged with the change-feed position it reflects.
//! A search whose library has moved on since starts a rebuild in the background and answers
//! from the index it has, so no search waits on one; only a library's very first search
//! waits for its index to be built. The index is rebuildable data (`design/database.md` §7).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use axum::Json;
use axum::extract::{Path, State};
use jewelcase_core::search::{Document, Index, Kind};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::albums::{self, AlbumSummary};
use super::artists::{self, ArtistSummary};
use super::page;
use super::query::Query;
use super::tracks::{self, TrackSummary};
use super::{Code, Id, Problem};
use crate::db::{Database, DbError};

/// Every library's search index.
#[derive(Default)]
pub struct Search {
    libraries: Mutex<HashMap<i64, Arc<Slot>>>,
}

#[derive(Default)]
struct Slot {
    built: RwLock<Option<Built>>,
    /// Held while building, so a library builds one index at a time.
    building: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Clone)]
struct Built {
    index: Arc<Index>,
    /// The library's change-feed position when the index was read.
    position: i64,
}

impl Search {
    /// The library's index, starting a rebuild if the library has changed since `position`.
    async fn index(
        &self,
        db: &Arc<Database>,
        library: i64,
        position: i64,
    ) -> Result<Arc<Index>, DbError> {
        let slot = self
            .libraries
            .lock()
            .unwrap()
            .entry(library)
            .or_default()
            .clone();
        let built = slot.built.read().unwrap().clone();
        if let Some(built) = built {
            if built.position != position
                && let Ok(guard) = slot.building.clone().try_lock_owned()
            {
                let (db, slot) = (db.clone(), slot.clone());
                tokio::spawn(async move {
                    match build(&db, library).await {
                        Ok(built) => *slot.built.write().unwrap() = Some(built),
                        Err(error) => {
                            tracing::warn!(%error, library, "cannot rebuild a search index")
                        }
                    }
                    drop(guard);
                });
            }
            return Ok(built.index);
        }
        // The first search waits for the index, and so does any that arrives meanwhile.
        let _guard = slot.building.lock().await;
        if let Some(built) = slot.built.read().unwrap().clone() {
            return Ok(built.index);
        }
        let built = build(db, library).await?;
        let index = built.index.clone();
        *slot.built.write().unwrap() = Some(built);
        Ok(index)
    }
}

/// Reads the library's names and indexes them.
async fn build(db: &Database, library: i64) -> Result<Built, DbError> {
    let (position, documents) = db
        .read(move |conn| {
            // The position first: a change landing between the two reads makes the index look
            // older than it is, so the next search rebuilds it again, never the reverse.
            let position = feed_position(conn, library)?;
            let documents = conn
                .prepare_cached(
                    "SELECT 0, id, title FROM tracks WHERE library_id = ?1 \
                     UNION ALL SELECT 1, id, title FROM albums WHERE library_id = ?1 AND title IS NOT NULL \
                     UNION ALL SELECT 2, id, name FROM artists WHERE library_id = ?1 AND name IS NOT NULL",
                )?
                .query_map([library], |row| {
                    Ok(Document {
                        kind: Kind::ALL[usize::from(row.get::<_, u8>(0)?)],
                        id: row.get(1)?,
                        name: row.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((position, documents))
        })
        .await?;
    // A second of CPU at full scale, so off the async workers.
    let index = tokio::task::spawn_blocking(move || Index::build(documents))
        .await
        .expect("building a search index does not panic");
    Ok(Built {
        index: Arc::new(index),
        position,
    })
}

fn feed_position(conn: &Connection, library: i64) -> rusqlite::Result<i64> {
    conn.prepare_cached("SELECT ifnull(max(seq), 0) FROM library_changes WHERE library_id = ?1")?
        .query_row([library], |row| row.get(0))
}

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
    State(search): State<Arc<Search>>,
    Path(library_id): Path<String>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, Problem> {
    let Id(library) = Id::parse(&library_id)?;
    let invalid = |detail| Problem::new(Code::ValidationFailed).detail(detail);
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
            Ok(Some((track_count, feed_position(conn, library)?)))
        })
        .await?
        .ok_or_else(|| Problem::new(Code::NotFound))?;
    let index = search.index(&db, library, position).await?;
    let found = index.search(&query.q, &kinds, limit);

    let sections = db
        .read(move |conn| {
            let mut albums_seen = HashMap::new();
            let mut sections = Vec::with_capacity(found.len());
            for section in found {
                let (kind, items) = match section.kind {
                    Kind::Track => (
                        "tracks",
                        page::by_ids(
                            conn,
                            tracks::SELECT,
                            "tracks t",
                            "t",
                            library,
                            &section.ids,
                            |row| {
                                let track = tracks::list_summary(conn, row, &mut albums_seen)?;
                                Ok(SearchResult::Track { track })
                            },
                        )?,
                    ),
                    Kind::Album => (
                        "albums",
                        page::by_ids(
                            conn,
                            albums::SELECT,
                            "albums al",
                            "al",
                            library,
                            &section.ids,
                            |row| {
                                Ok(SearchResult::Album {
                                    album: albums::summary(conn, row)?,
                                })
                            },
                        )?,
                    ),
                    Kind::Artist => (
                        "artists",
                        page::by_ids(
                            conn,
                            artists::SELECT,
                            "artists ar",
                            "ar",
                            library,
                            &section.ids,
                            |row| {
                                Ok(SearchResult::Artist {
                                    artist: artists::summary(row)?,
                                })
                            },
                        )?,
                    ),
                };
                // Gone since the index was built: nothing of it is left to show.
                if !items.is_empty() {
                    sections.push(Section {
                        kind,
                        items,
                        total: section.total,
                    });
                }
            }
            Ok(sections)
        })
        .await?;

    Ok(Json(SearchResponse {
        query: query.q,
        top: sections.first().map(|section| section.items[0].clone()),
        sections,
        near_misses: Vec::new(),
        library_empty: track_count == 0,
    }))
}

#[cfg(test)]
mod tests;
