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
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use axum::Router;
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use super::super::tests::{app, send};
    use super::*;
    use crate::db::libraries;

    /// The tracks fixture (`tracks::tests::fixture`), with library 2's track counted, and an
    /// empty library 3.
    async fn app_with_fixture() -> (tempfile::TempDir, Arc<Database>, Router) {
        let (temp, db, app) = app("");
        db.write(|tx| {
            tracks::tests::fixture(tx)?;
            tx.execute("UPDATE libraries SET track_count = 1 WHERE id = 2", [])?;
            libraries::create(tx, Some(3), "Empty", &[Path::new("/empty")], &[])?;
            Ok(())
        })
        .await
        .unwrap();
        (temp, db, app)
    }

    async fn json(app: &Router, uri: &str) -> (StatusCode, Value) {
        let (status, _, body) = send(app.clone(), "GET", uri).await;
        (status, serde_json::from_slice(&body).unwrap())
    }

    /// The item with `id` from a browse list, as the list shows it.
    async fn listed(app: &Router, list: &str, id: &str) -> Value {
        let (_, page) = json(app, &format!("/api/v1/libraries/1/{list}?limit=1000")).await;
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap()
            .clone()
    }

    /// Each section's type and item IDs, in order.
    fn sections(body: &Value) -> Vec<(String, Vec<String>)> {
        body["sections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|section| {
                let kind = section["type"].as_str().unwrap().to_owned();
                let key = kind.trim_end_matches('s').to_owned();
                let ids = section["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item[&key]["id"].as_str().unwrap().to_owned())
                    .collect();
                (kind, ids)
            })
            .collect()
    }

    #[tokio::test]
    async fn results_look_like_the_browse_lists() {
        let (_temp, _db, app) = app_with_fixture().await;
        let (status, body) = json(&app, "/api/v1/libraries/1/search?q=beatles").await;
        assert_eq!(status, StatusCode::OK);
        let artist = listed(&app, "artists", "10").await;
        assert_eq!(
            body,
            json!({
                "query": "beatles",
                "top": { "type": "artist", "artist": artist },
                "sections": [{ "type": "artists", "items": [{ "type": "artist", "artist": artist }], "total": 1 }],
                "nearMisses": [],
                "libraryEmpty": false,
            })
        );

        // A typo, and a track exactly as `listTracks` shows it.
        let (_, body) = json(&app, "/api/v1/libraries/1/search?q=come%20togther").await;
        assert_eq!(
            body["top"],
            json!({ "type": "track", "track": listed(&app, "tracks", "1001").await })
        );
    }

    #[tokio::test]
    async fn sections_follow_the_core_and_the_filters() {
        let (_temp, _db, app) = app_with_fixture().await;
        let (_, body) = json(&app, "/api/v1/libraries/1/search?q=kid").await;
        assert_eq!(
            sections(&body),
            [("albums".to_owned(), vec!["102".to_owned()])]
        );
        let (_, body) = json(
            &app,
            "/api/v1/libraries/1/search?q=kid&types=tracks,playlists",
        )
        .await;
        assert_eq!(body["sections"], json!([]));
        assert_eq!(body["top"], Value::Null);
        // Equally exact everywhere: tracks, then albums, then artists.
        let (_, body) = json(
            &app,
            "/api/v1/libraries/2/search?q=elsewhere&sectionLimit=1",
        )
        .await;
        assert_eq!(
            sections(&body),
            [
                ("tracks".to_owned(), vec!["2001".to_owned()]),
                ("albums".to_owned(), vec!["201".to_owned()]),
                ("artists".to_owned(), vec!["20".to_owned()]),
            ]
        );
    }

    #[tokio::test]
    async fn only_the_callers_library_is_searched() {
        let (_temp, _db, app) = app_with_fixture().await;
        let (_, body) = json(&app, "/api/v1/libraries/1/search?q=elsewhere").await;
        assert_eq!(body["sections"], json!([]));
        assert_eq!(body["libraryEmpty"], false);
        let (_, body) = json(&app, "/api/v1/libraries/3/search?q=elsewhere").await;
        assert_eq!(body["sections"], json!([]));
        assert_eq!(body["libraryEmpty"], true, "no music yet, not no match");
    }

    #[tokio::test]
    async fn the_index_follows_the_library() {
        let (_temp, db, app) = app_with_fixture().await;
        let uri = "/api/v1/libraries/1/search?q=octopus";
        let (_, body) = json(&app, uri).await;
        assert_eq!(body["sections"], json!([]));
        db.write(|tx| {
            tx.execute_batch(
                "INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size, file_mtime,
                                     title, sort_key, artist_sort_key, album_sort_key, codec,
                                     container, lossless, sample_rate_hz, channels, duration_us,
                                     added_at, updated_at)
                 SELECT 1007, 1, 101, root_id, 'o.flac', 1, 0, 'Octopus''s Garden', x'6f', x'',
                        x'', 'flac', 'flac', 1, 44100, 2, 1, 0, 0 FROM tracks WHERE id = 1001;
                 INSERT INTO library_changes (library_id, entity_type, entity_id, op, at)
                 VALUES (1, 'track', 1007, 'upsert', 0);",
            )
        })
        .await
        .unwrap();
        // The search that notices the change answers from the old index while a new one builds.
        for _ in 0..100 {
            let (_, body) = json(&app, uri).await;
            if body["sections"] != json!([]) {
                assert_eq!(
                    sections(&body),
                    [("tracks".to_owned(), vec!["1007".to_owned()])]
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the index never caught up");
    }

    #[tokio::test]
    async fn problems() {
        let (_temp, _db, app) = app_with_fixture().await;
        for (uri, status) in [
            ("/api/v1/libraries/9/search?q=a", StatusCode::NOT_FOUND),
            (
                "/api/v1/libraries/1/search",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/api/v1/libraries/1/search?q=",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/api/v1/libraries/1/search?q=a&sectionLimit=0",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/api/v1/libraries/1/search?q=a&sectionLimit=51",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                "/api/v1/libraries/1/search?q=a&types=songs",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            // Filters not built yet are refused rather than ignored.
            (
                "/api/v1/libraries/1/search?q=a&genre=rock",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let (actual, content_type, _) = send(app.clone(), "GET", uri).await;
            assert_eq!(actual, status, "{uri}");
            assert_eq!(
                content_type.as_deref(),
                Some("application/problem+json"),
                "{uri}"
            );
        }
    }
}
