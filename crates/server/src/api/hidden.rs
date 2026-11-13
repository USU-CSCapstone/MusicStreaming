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
use crate::db::feed::{self, Kind, Op};
use crate::db::marks::{self, Mark};
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
                // Unliked first, so a plugin applying the marks in order ends on the hide.
                let unliked = playlists::unlike_in(tx, user, library, track)?;
                let hid = tx
                    .prepare_cached(
                        "INSERT OR IGNORE INTO hidden_tracks (user_id, library_id, track_id, \
                         hidden_at) VALUES (?1, ?2, ?3, ?4)",
                    )?
                    .execute(params![user, library, track, now_ms()])?
                    > 0;
                if hid {
                    marks::record(tx, user, library, [track], Mark::Hidden)?;
                    feed::record_account(
                        tx,
                        user,
                        library,
                        Kind::HiddenTrack,
                        track,
                        0,
                        Op::Upsert,
                    )?;
                }
                Ok((hid, unliked))
            })
            .await?;
        if unliked.is_some() {
            playlists::announce_liked(&hub, user, library, [track], false);
        }
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
    // A hidden track is never liked, so none is marked so; every one is marked hidden.
    let page = db
        .read(move |conn| {
            let mut page = request.read(conn, &source, tracks::list_summary)?;
            tracks::mark_hidden(conn, user, library, &mut page.items)?;
            Ok(page)
        })
        .await?;
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

/// `source`, a list of tracks `t`, without the tracks `user` hid in `library`
/// ([`leave_out_ids`]).
pub fn leave_out(
    conn: &Connection,
    source: &mut Source,
    user: i64,
    library: i64,
) -> rusqlite::Result<()> {
    let hidden: Vec<i64> = ids(conn, user, library)?.into_iter().collect();
    leave_out_ids(conn, source, "t.id", &hidden)
}

/// `source`, a list whose rows `column` names, such as `al.id`, without the rows `ids`, which
/// are few: what the caller hid, or hid whole. Each row is one lookup in them. A stored total
/// loses those of them the list holds, counted by reading the list for those alone, so the total
/// stays a stored count. Over a table, or a lead of one table such as a tag's, that is a lookup
/// per ID, so it costs what they do. A lead SQLite cannot look into by ID, a compound one such as
/// an artist's owned and featured albums together, is read through once instead, as the page
/// itself reads it to sort it, so the count costs no more than the page. Nothing changes where
/// there are none, the common case.
pub fn leave_out_ids(
    conn: &Connection,
    source: &mut Source,
    column: &str,
    ids: &[i64],
) -> rusqlite::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let ids = Value::from(serde_json::to_string(ids).expect("IDs serialize"));
    if let Total::Stored(total) = source.total {
        let sql = format!(
            "SELECT count(*) FROM {} WHERE {} AND {column} IN (SELECT value FROM json_each(?))",
            source.from, source.filter
        );
        let params = source.params.iter().cloned().chain([ids.clone()]);
        let held: i64 =
            conn.prepare_cached(&sql)?.query_row(params_from_iter(params), |row| row.get(0))?;
        source.total = Total::Stored(total - held);
    }
    source.filter += &format!(" AND {column} NOT IN (SELECT value FROM json_each(?))");
    source.params.push(ids);
    Ok(())
}

/// The user's hidden tracks, as the queries of [`albums`], [`artists`] and [`tags`] start from.
const HID: &str =
    "WITH hid AS (SELECT track_id FROM hidden_tracks WHERE user_id = ?1 AND library_id = ?2) ";

/// The albums of `library` every track of which `user` hid, which are gone from their library
/// with them (`requirements/conventions.md` §8). Only the albums of hidden tracks can be, and
/// each is kept unless a probe of `tracks_by_album_order` finds a track left, which it stops at.
fn albums(conn: &Connection, user: i64, library: i64) -> rusqlite::Result<Vec<i64>> {
    wholly(
        conn,
        user,
        library,
        "SELECT DISTINCT t.album_id FROM hid CROSS JOIN tracks t ON t.id = hid.track_id \
         WHERE NOT EXISTS (SELECT 1 FROM tracks o \
                           WHERE o.album_id = t.album_id AND o.id NOT IN hid)",
    )
}

/// The artists of `library` every track of which `user` hid: those that credit them and those
/// on their albums, each probed through its index until one is found left, as for [`albums`].
fn artists(conn: &Connection, user: i64, library: i64) -> rusqlite::Result<Vec<i64>> {
    wholly(
        conn,
        user,
        library,
        "SELECT c.artist_id FROM ( \
           SELECT ta.artist_id FROM hid CROSS JOIN track_artists ta ON ta.track_id = hid.track_id \
           UNION \
           SELECT ta.artist_id FROM hid \
             CROSS JOIN track_album_artists ta ON ta.track_id = hid.track_id) c \
         WHERE c.artist_id IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM track_artists o \
                           WHERE o.artist_id = c.artist_id AND o.track_id NOT IN hid) \
           AND NOT EXISTS (SELECT 1 FROM track_album_artists o \
                           WHERE o.artist_id = c.artist_id AND o.track_id NOT IN hid)",
    )
}

/// The tags of `library` every track carrying which `user` hid, each probed through
/// `track_tags_by_tag`, as for [`albums`].
pub fn tags(conn: &Connection, user: i64, library: i64) -> rusqlite::Result<Vec<i64>> {
    wholly(
        conn,
        user,
        library,
        "SELECT DISTINCT tt.tag_id FROM hid CROSS JOIN track_tags tt ON tt.track_id = hid.track_id \
         WHERE NOT EXISTS (SELECT 1 FROM track_tags o \
                           WHERE o.tag_id = tt.tag_id AND o.track_id NOT IN hid)",
    )
}

/// The IDs `select`, a query after [`HID`], finds for `user` in `library`.
fn wholly(conn: &Connection, user: i64, library: i64, select: &str) -> rusqlite::Result<Vec<i64>> {
    conn.prepare_cached(&format!("{HID}{select}"))?
        .query_map([user, library], |row| row.get(0))?
        .collect()
}

/// Unhides `track` for `user` in `library`, in the writer, and says whether it was hidden.
pub fn unhide_in(tx: &Connection, user: i64, library: i64, track: i64) -> rusqlite::Result<bool> {
    let deleted = tx
        .prepare_cached(
            "DELETE FROM hidden_tracks WHERE user_id = ?1 AND library_id = ?2 AND track_id = ?3",
        )?
        .execute(params![user, library, track])?
        > 0;
    if deleted {
        marks::record(tx, user, library, [track], Mark::Unhidden)?;
        feed::record_account(tx, user, library, Kind::HiddenTrack, track, 0, Op::Delete)?;
    }
    Ok(deleted)
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

mod losses;
pub use losses::{LibraryLoss, Losses, losses, playlists};

#[cfg(test)]
mod tests;
