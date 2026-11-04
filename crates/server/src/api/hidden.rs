//! Hidden tracks: `hideTrack`, `unhideTrack` and `listHiddenTracks` (`api/openapi.yaml`). A
//! hidden track is gone from the caller's library and shown only in Hidden Songs, a page at a
//! time, newest first (`requirements/conventions.md` §8). Hiding a liked track unlikes it, as
//! liking a hidden one unhides it (`playlists::like`), so the two never contradict each other.
//! Each change is announced to the caller's devices once it is committed.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rusqlite::types::Value;
use rusqlite::{Connection, params, params_from_iter};
use serde::Deserialize;

use super::extract::{Path, Query};
use super::page::{Order, Page, Request, Sort, Source, Total, Unknown};
use super::playlists;
use super::realtime::{Hub, ServerMessage};
use super::scope::Scope;
use super::tracks::{self, TrackSummary};
use super::{Id, Problem};
use crate::db::{Database, now_ms};

/// Hidden Songs' order, by when each track was hidden, through `hidden_tracks_by_time`.
const HIDDEN_AT: Sort =
    Sort { label: "hidden.time", columns: &["h.hidden_at", "h.track_id"], unknown: Unknown::Never };

/// The parameters `listHiddenTracks` supports. Any other answers `422` rather than being ignored.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListQuery {
    order: Option<Order>,
    cursor: Option<String>,
    limit: Option<u32>,
}

/// `hideTrack`: hides the track from the caller in this library, and unlikes it. A track the
/// library does not have is not found; one hidden already stays as it was.
pub async fn hide(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    playlists::settle(async move {
        let (hid, unliked) = db
            .try_write(move |tx| {
                let found: bool = tx
                    .prepare_cached(
                        "SELECT EXISTS (SELECT 1 FROM tracks WHERE library_id = ?1 AND id = ?2)",
                    )?
                    .query_row([library, track], |row| row.get(0))?;
                if !found {
                    return Err(Problem::not_found());
                }
                let hid = tx
                    .prepare_cached(
                        "INSERT OR IGNORE INTO hidden_tracks (user_id, library_id, track_id, \
                         hidden_at) VALUES (?1, ?2, ?3, ?4)",
                    )?
                    .execute(params![user, library, track, now_ms()])?
                    > 0;
                Ok((hid, playlists::unlike_in(tx, user, library, track)?))
            })
            .await?;
        playlists::announce(&hub, user, unliked);
        if hid {
            announce(&hub, user, library, [track], true);
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `unhideTrack`: brings the track back into the caller's library. One not hidden is already
/// so, which is the answer.
pub async fn unhide(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    playlists::settle(async move {
        if db.write(move |tx| unhide_in(tx, user, library, track)).await? {
            announce(&hub, user, library, [track], false);
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `listHiddenTracks`: Hidden Songs, the tracks the caller hid in this library, newest first
/// unless `order` says `asc`, a page at a time.
pub async fn list(
    State(db): State<Arc<Database>>,
    Scope { user, library }: Scope,
    Query(query): Query<ListQuery>,
) -> Result<Json<Page<TrackSummary>>, Problem> {
    let order = query.order.unwrap_or(Order::Desc);
    let request = Request::new(&HIDDEN_AT, order, query.cursor, query.limit)?;
    // Counted for every page: a user hides few of a library's tracks, all in one index range.
    let source = Source {
        select: tracks::SELECT,
        from: "hidden_tracks h CROSS JOIN tracks t ON t.library_id = h.library_id \
               AND t.id = h.track_id"
            .to_owned(),
        joins: tracks::JOINS,
        filter: "h.user_id = ? AND h.library_id = ?".to_owned(),
        params: vec![Value::from(user), Value::from(library)],
        total: Total::Counted,
    };
    // A hidden track is never liked, so none is marked.
    let page = db.read(move |conn| request.read(conn, &source, tracks::list_summary)).await?;
    Ok(Json(page))
}

/// The tracks `user` hid in `library`. A user hides few of a library's tracks, so they are held
/// as a set, which a search consults for each match.
pub fn ids(conn: &Connection, user: i64, library: i64) -> rusqlite::Result<HashSet<i64>> {
    conn.prepare_cached(
        "SELECT track_id FROM hidden_tracks WHERE user_id = ?1 AND library_id = ?2",
    )?
    .query_map([user, library], |row| row.get(0))?
    .collect()
}

/// `source`, a list of tracks `t`, without the tracks `user` hid in `library`. Each row is one
/// probe of `hidden_tracks`. A stored total loses the hidden tracks the list holds, counted by
/// reading the list for those alone, which costs what they cost to find rather than the list, so
/// a total stays a stored count. Nothing changes for a user who hid nothing in it.
pub fn leave_out(
    conn: &Connection,
    source: &mut Source,
    user: i64,
    library: i64,
) -> rusqlite::Result<()> {
    let any: bool = conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM hidden_tracks WHERE user_id = ?1 AND library_id = ?2)",
        )?
        .query_row([user, library], |row| row.get(0))?;
    if !any {
        return Ok(());
    }
    if let Total::Stored(total) = source.total {
        let sql = format!(
            "SELECT count(*) FROM {} WHERE {} AND t.id IN \
             (SELECT track_id FROM hidden_tracks WHERE user_id = ? AND library_id = ?)",
            source.from, source.filter
        );
        let params = source.params.iter().cloned().chain([Value::from(user), Value::from(library)]);
        let hidden: i64 =
            conn.prepare_cached(&sql)?.query_row(params_from_iter(params), |row| row.get(0))?;
        source.total = Total::Stored(total - hidden);
    }
    source.filter += " AND NOT EXISTS (SELECT 1 FROM hidden_tracks h WHERE h.user_id = ? \
                      AND h.library_id = t.library_id AND h.track_id = t.id)";
    source.params.push(Value::from(user));
    Ok(())
}

/// Unhides `track` for `user` in `library`, and says whether it was hidden.
pub fn unhide_in(conn: &Connection, user: i64, library: i64, track: i64) -> rusqlite::Result<bool> {
    let deleted = conn
        .prepare_cached(
            "DELETE FROM hidden_tracks WHERE user_id = ?1 AND library_id = ?2 AND track_id = ?3",
        )?
        .execute(params![user, library, track])?;
    Ok(deleted > 0)
}

/// Tells `user`'s devices that `tracks` of `library` were hidden, or unhidden. Call it once the
/// change is committed.
pub fn announce(
    hub: &Hub,
    user: i64,
    library: i64,
    tracks: impl IntoIterator<Item = i64>,
    hidden: bool,
) {
    for track in tracks {
        let message =
            ServerMessage::TrackHidden { library_id: Id(library), track_id: Id(track), hidden };
        hub.broadcast(user, &message);
    }
}

#[cfg(test)]
mod tests;
