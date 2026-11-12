//! Recent searches: `listRecentSearches`, `recordRecentSearch`, `deleteRecentSearch`, and
//! `clearRecentSearches` (`api/openapi.yaml`, Search).
//!
//! What the search box offers when it is empty (`requirements/search.md` §6). They are each
//! user's own and kept per library, one entry per query. They are kept only for a recent
//! window: the [`KEPT`] newest, none older than [`WINDOW_MS`]. Past that a search is deleted:
//! one pushed out by newer ones as they are recorded, and one aged out by the hourly sweep
//! (`crate::db::retention`), which the list leaves out until then. So there is no permanent
//! search history.
//!
//! A client records a search once it settles: when the user acts on a result, submits it, or
//! leaves it after it has stood a moment, under an ID it may choose, as a device that records one
//! offline does. Searching the same words again, ignoring case and accents, replaces the earlier
//! entry, so it moves to the top under a new ID. The rows themselves are written by the code a
//! device runs too (`jewelcase_catalog::mutate`), and each change goes to the account feed, so a
//! device keeps them (`design/database.md` §5).
//!
//! For a user who shares their searches with a plugin, each one recorded, with how much it found,
//! and each one they remove, is noted for the `searched` hook (`0005_activity.sql`). Nothing else
//! reads what a search found, so it is counted only for such a user. Only explicit removal
//! asks a plugin to forget: a search replaced by the same words, pushed out by newer ones, or
//! aged out of the window does not (`requirements/search.md` §6).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use jewelcase_catalog::mutate::{Effect, Writer, check_query};
use jewelcase_catalog::recent_searches::{self, RecentSearch, RecentSearches};
use rusqlite::{Transaction, params};
use serde::Deserialize;

use super::extract::{self, Path};
use super::playlists::record as feed;
use super::scope::Scope;
use super::search::{Indexes, totals};
use super::{AnyId, Id, Problem};
use crate::db::retention::WINDOW_MS;
use crate::db::{Database, new_id, now_ms};

/// The spec's `RecentSearchCreate`.
#[derive(Deserialize)]
pub struct Create {
    query: String,
    selected: Option<Selected>,
    /// The ID the client chose, which a device that records offline uses until the record syncs.
    id: Option<Id>,
}

/// The result a search's user acted on, as the request names it.
#[derive(Deserialize)]
struct Selected {
    #[serde(rename = "type")]
    kind: String,
    id: AnyId,
}

/// Whether user `?1` shares their searches with any plugin.
const SHARES: &str = "EXISTS (SELECT 1 FROM plugin_connections \
                      WHERE user_id = ?1 AND searches_shared_at IS NOT NULL)";

/// What a user did with one of their recent searches, as the `searched` hook is told it.
enum Noted {
    /// Recorded it, and it found this many tracks, albums, and artists.
    Searched([usize; 3]),
    /// Removed it.
    Forgotten,
}

/// Notes what `user` did with `search`, for the plugins they share their searches with; nothing
/// if they share with none.
fn note(
    tx: &Transaction,
    user: i64,
    library: i64,
    search: i64,
    noted: Noted,
    now: i64,
) -> rusqlite::Result<()> {
    let (kind, [tracks, albums, artists]) = match noted {
        Noted::Searched(found) => ("searched", found.map(Some)),
        Noted::Forgotten => ("forgotten", [None; 3]),
    };
    tx.execute(
        &format!(
            "INSERT INTO search_events (user_id, library_id, kind, search_id, found_tracks, \
                                        found_albums, found_artists, created_at) \
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8 WHERE {SHARES}"
        ),
        params![user, library, kind, search, tracks, albums, artists, now],
    )?;
    Ok(())
}

/// Runs `write` with a writer of `user`'s searches in `library`, whose changes go to the account
/// feed and, for a search recorded with what it `found` or one the user removed, to the plugins
/// they share their searches with. A search replaced or pushed out is not noted: only explicit
/// removal asks a plugin to forget.
fn writing<T>(
    tx: &Transaction,
    user: i64,
    library: i64,
    found: Option<[usize; 3]>,
    write: impl FnOnce(&mut Writer) -> Result<T, jewelcase_catalog::mutate::Error>,
) -> Result<T, Problem> {
    let now = now_ms();
    let mut effects = |effect| {
        feed(tx, user, library, effect)?;
        match effect {
            // Someone who shared nothing when it was counted is not sent it, even if they have
            // turned sharing on since.
            Effect::Search { id, kept: true, .. } => match found {
                Some(found) => note(tx, user, library, id, Noted::Searched(found), now),
                None => Ok(()),
            },
            Effect::Search { id, removed: true, .. } => {
                note(tx, user, library, id, Noted::Forgotten, now)
            }
            _ => Ok(()),
        }
    };
    Ok(write(&mut Writer::new(tx, user, library, now, &mut effects))?)
}

/// The caller's recent searches in `library`, newest first.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
) -> Result<Json<RecentSearches>, Problem> {
    let since = now_ms() - WINDOW_MS;
    let items = db.read(move |conn| recent_searches::list(conn, user, library, since)).await?;
    Ok(Json(items))
}

/// Records a search as the caller's newest in `library`, under the `id` it chose or a new one,
/// replacing an earlier one of the same words, and returns it as `201`. A `selected` result not
/// in `library` and an `id` that is not a positive 63-bit integer are `422`; an `id` any user's
/// search has is `409 id_taken`.
pub async fn record(
    State(db): State<Arc<Database>>,
    State(indexes): State<Arc<Indexes>>,
    Scope { user, library }: Scope,
    extract::Json(Create { query, selected, id }): extract::Json<Create>,
) -> Result<(StatusCode, Json<RecentSearch>), Problem> {
    let (query, _) = check_query(&query)?;
    let selected = match selected {
        None => None,
        Some(Selected { kind, id: AnyId(Some(id)) }) => Some((kind, id)),
        Some(Selected { id: AnyId(None), .. }) => {
            return Err(Problem::invalid("selected.id is not an ID."));
        }
    };
    // Only the `searched` hook reads what a search found, so it is counted only for a user who
    // shares their searches.
    let shares = db
        .read(move |conn| conn.query_row(&format!("SELECT {SHARES}"), [user], |row| row.get(0)))
        .await?;
    let found = if shares { Some(totals(&db, &indexes, library, &query).await?) } else { None };
    let recorded = db
        .try_write(move |tx| {
            let id = match id {
                Some(Id(id)) => id,
                None => new_id()?,
            };
            let selected = selected.as_ref().map(|(kind, id)| (kind.as_str(), *id));
            writing(tx, user, library, found, |w| w.record_search(id, &query, selected))?;
            Ok::<_, Problem>(recent_searches::get(tx, id)?)
        })
        .await?;
    Ok((StatusCode::CREATED, Json(recorded)))
}

/// Removes one of the caller's recent searches in `library`, and has the plugins they share
/// their searches with forget it.
pub async fn delete(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
    Path((_, Id(search))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    let deleted = db
        .try_write(move |tx| writing(tx, user, library, None, |w| w.delete_search(search)))
        .await?;
    // Someone else's is as absent as one that never was (`requirements/users.md` §10).
    if !deleted {
        return Err(Problem::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Removes all the caller's recent searches in `library`, and has the plugins they share their
/// searches with forget each.
pub async fn clear(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
) -> Result<StatusCode, Problem> {
    db.try_write(move |tx| writing(tx, user, library, None, |w| w.clear_searches())).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
