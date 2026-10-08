//! Recent searches: `listRecentSearches`, `recordRecentSearch`, `deleteRecentSearch`, and
//! `clearRecentSearches` (`api/openapi.yaml`, Search).
//!
//! What the search box offers when it is empty (`requirements/search.md` §6). They are each
//! user's own and kept per library, one entry per query. They are kept only for a recent
//! window: the [`KEPT`] newest, none older than [`WINDOW_MS`]. Past that a search is deleted,
//! not hidden. Every write sweeps away what has aged out for everyone, so there is no permanent
//! search history.
//!
//! A client records a search once it settles: when the user acts on a result, submits it, or
//! leaves it after it has stood a moment. Searching the same words again, ignoring case and
//! accents, replaces the earlier entry, so it moves to the top under a new ID.
//!
//! For a user who shares their searches with a plugin, each one recorded, and each one they
//! remove, is noted for the `searched` hook (`0016_plugin_searched.sql`). Only explicit removal
//! asks a plugin to forget: a search replaced by the same words, pushed out by newer ones, or
//! aged out of the window does not (`requirements/search.md` §6).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use jewelcase_core::search::words;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::{Deserialize, Serialize};

use super::authenticate::Caller;
use super::extract::{Json as Body, Path};
use super::search::{Indexes, totals};
use super::sql::timestamp;
use super::{Id, Problem};
use crate::db::{Database, now_ms};

/// The most recent searches kept per user and library.
const KEPT: i64 = 20;
/// How long a recent search is kept: 30 days.
const WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// The longest query kept, in bytes.
const MAX_QUERY: usize = 500;

/// The spec's `RecentSearch`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentSearch {
    id: Id,
    query: String,
    selected: Option<EntityRef>,
    searched_at: String,
}

/// The spec's `EntityRef`, for the kinds of result search returns.
#[derive(Serialize)]
pub struct EntityRef {
    #[serde(rename = "type")]
    kind: &'static str,
    id: Id,
}

/// The spec's `RecentSearchCreate`.
#[derive(Deserialize)]
pub struct Create {
    query: String,
    selected: Option<Selected>,
}

#[derive(Deserialize)]
struct Selected {
    #[serde(rename = "type")]
    kind: String,
    id: String,
}

/// The kinds of result a search returns, and the table each is in.
const KINDS: [(&str, &str); 3] = [("track", "tracks"), ("album", "albums"), ("artist", "artists")];

const COLUMNS: &str = concat!(
    "id, query, selected_type, selected_id, ",
    timestamp!("searched_at"),
    " FROM recent_searches"
);

fn row(row: &Row) -> rusqlite::Result<RecentSearch> {
    let kind: Option<String> = row.get(2)?;
    let kind = kind.and_then(|kind| KINDS.iter().find(|(k, _)| *k == kind).map(|(k, _)| *k));
    let selected = match (kind, row.get::<_, Option<i64>>(3)?) {
        (Some(kind), Some(id)) => Some(EntityRef { kind, id: Id(id) }),
        _ => None,
    };
    Ok(RecentSearch { id: Id(row.get(0)?), query: row.get(1)?, selected, searched_at: row.get(4)? })
}

/// Deletes every search, anyone's, that has aged out of the window, and what the `searched`
/// hook noted about searches as old.
fn sweep(tx: &Transaction, now: i64) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM recent_searches WHERE searched_at < ?1", [now - WINDOW_MS])?;
    tx.execute("DELETE FROM search_events WHERE at < ?1", [now - WINDOW_MS])?;
    Ok(())
}

/// Notes that `user` recorded or removed `search`, for the plugins they share their searches
/// with; nothing if they share with none.
fn note(
    tx: &Transaction,
    user: i64,
    library: i64,
    kind: &str,
    search: i64,
    now: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO search_events (user_id, library_id, kind, search_id, at) \
         SELECT ?1, ?2, ?3, ?4, ?5 \
         WHERE EXISTS (SELECT 1 FROM plugin_search_sharing WHERE user_id = ?1)",
        params![user, library, kind, search, now],
    )?;
    Ok(())
}

pub async fn list(
    State(db): State<Arc<Database>>,
    caller: Caller,
    Path(Id(library)): Path<Id>,
) -> Result<Json<serde_json::Value>, Problem> {
    let user = caller.user;
    let items = db
        .read(move |conn| {
            conn.prepare_cached(&format!(
                "SELECT {COLUMNS} WHERE user_id = ?1 AND library_id = ?2 AND searched_at >= ?3 \
                 ORDER BY searched_at DESC, id DESC LIMIT ?4"
            ))?
            .query_map(params![user, library, now_ms() - WINDOW_MS, KEPT], row)?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await?;
    Ok(Json(serde_json::json!({ "items": items })))
}

pub async fn record(
    State(db): State<Arc<Database>>,
    State(indexes): State<Arc<Indexes>>,
    caller: Caller,
    Path(Id(library)): Path<Id>,
    Body(Create { query, selected }): Body<Create>,
) -> Result<(StatusCode, Json<RecentSearch>), Problem> {
    let query = query.trim().to_owned();
    if query.len() > MAX_QUERY {
        return Err(Problem::invalid(format!("query must be at most {MAX_QUERY} bytes.")));
    }
    let folded = words(&query).join(" ");
    if folded.is_empty() {
        return Err(Problem::invalid("query has nothing in it to search for."));
    }
    let selected = match selected {
        None => None,
        Some(Selected { kind, id }) => {
            let Some((kind, table)) = KINDS.iter().find(|(k, _)| *k == kind) else {
                return Err(Problem::invalid("selected must be a track, album, or artist."));
            };
            let Some(Id(id)) = Id::canonical(&id) else {
                return Err(Problem::invalid("selected.id is not an ID."));
            };
            Some((*kind, *table, id))
        }
    };
    let [tracks, albums, artists] = totals(&db, &indexes, library, &query).await?;
    let found = [tracks, albums, artists].map(|n| n as i64);
    let user = caller.user;
    let recorded = db
        .write(move |tx| {
            if let Some((_, table, id)) = selected
                && !in_library(tx, table, library, id)?
            {
                return Ok(None);
            }
            let now = now_ms();
            sweep(tx, now)?;
            tx.execute(
                "DELETE FROM recent_searches WHERE user_id = ?1 AND library_id = ?2 AND folded = ?3",
                params![user, library, folded],
            )?;
            let (kind, id) = selected.map(|(kind, _, id)| (kind, id)).unzip();
            let id: i64 = tx.query_row(
                "INSERT INTO recent_searches (user_id, library_id, query, folded, selected_type, \
                                              selected_id, searched_at, found_tracks, \
                                              found_albums, found_artists) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) RETURNING id",
                params![user, library, query, folded, kind, id, now, found[0], found[1], found[2]],
                |row| row.get(0),
            )?;
            note(tx, user, library, "searched", id, now)?;
            // Only the newest are kept.
            tx.execute(
                "DELETE FROM recent_searches WHERE user_id = ?1 AND library_id = ?2 AND id NOT IN \
                 (SELECT id FROM recent_searches WHERE user_id = ?1 AND library_id = ?2 \
                  ORDER BY searched_at DESC, id DESC LIMIT ?3)",
                params![user, library, KEPT],
            )?;
            tx.query_row(&format!("SELECT {COLUMNS} WHERE id = ?1"), [id], row).map(Some)
        })
        .await?;
    let recorded = recorded.ok_or_else(|| Problem::invalid("selected is not in this library."))?;
    Ok((StatusCode::CREATED, Json(recorded)))
}

/// Whether `table` holds the entity `id` in `library`.
fn in_library(conn: &Connection, table: &str, library: i64, id: i64) -> rusqlite::Result<bool> {
    let sql = format!("SELECT 1 FROM {table} WHERE id = ?1 AND library_id = ?2");
    Ok(conn.prepare_cached(&sql)?.query_row(params![id, library], |_| Ok(())).optional()?.is_some())
}

pub async fn delete(
    State(db): State<Arc<Database>>,
    caller: Caller,
    Path((Id(library), Id(search))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    let user = caller.user;
    let deleted = db
        .write(move |tx| {
            let now = now_ms();
            sweep(tx, now)?;
            let deleted = tx.execute(
                "DELETE FROM recent_searches WHERE id = ?1 AND user_id = ?2 AND library_id = ?3",
                params![search, user, library],
            )?;
            if deleted > 0 {
                note(tx, user, library, "forgotten", search, now)?;
            }
            Ok(deleted)
        })
        .await?;
    // Someone else's is as absent as one that never was (`requirements/users.md` §10).
    if deleted == 0 {
        return Err(Problem::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn clear(
    State(db): State<Arc<Database>>,
    caller: Caller,
    Path(Id(library)): Path<Id>,
) -> Result<StatusCode, Problem> {
    let user = caller.user;
    db.write(move |tx| {
        let now = now_ms();
        sweep(tx, now)?;
        let cleared: Vec<i64> = tx
            .prepare(
                "DELETE FROM recent_searches WHERE user_id = ?1 AND library_id = ?2 RETURNING id",
            )?
            .query_map(params![user, library], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for search in cleared {
            note(tx, user, library, "forgotten", search, now)?;
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
