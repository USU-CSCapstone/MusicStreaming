//! `likeTrack` and `unlikeTrack` (`api/openapi.yaml`): a like puts a track at the top of the
//! caller's Liked Songs, made again if they deleted it, and an unlike takes it out
//! (`requirements/playlists.md` §11). Neither is checked against a version: each says whether a
//! track is liked, not where it sits, so it applies whatever else changed the playlist. Each is
//! announced to the caller's devices as an edit is. Liking a hidden track unhides it, and hiding
//! one unlikes it ([`take_out`]), so the two never contradict each other
//! (`requirements/conventions.md` §8). And how an edit's adds apply to a Liked Songs
//! ([`only_new`]).

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use jewelcase_core::playlist::Op;
use rusqlite::{Connection, params};

use super::announce::Change;
use super::{edit, representation};
use crate::api::extract::Path;
use crate::api::hidden;
use crate::api::realtime::Hub;
use crate::api::scope::Scope;
use crate::api::{Id, Problem};
use crate::db::{Database, liked_songs, now_ms};

/// `likeTrack`: puts the track at the top of the caller's Liked Songs, unless it is in it
/// already, making the playlist again, pinned, if they deleted it, and unhides it. A track the
/// library does not have is not found.
pub async fn like(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    super::settle(async move {
        let (change, unhidden) = db
            .try_write(move |tx| {
                let found: bool = tx
                    .prepare_cached(
                        "SELECT EXISTS (SELECT 1 FROM tracks WHERE library_id = ?1 AND id = ?2)",
                    )?
                    .query_row([library, track], |row| row.get(0))?;
                if !found {
                    return Err(Problem::not_found());
                }
                let unhidden = hidden::unhide_in(tx, user, library, track)?;
                let now = now_ms();
                let (id, version) = match liked_songs::find(tx, user, library)? {
                    Some(found) => found,
                    None => {
                        tx.prepare_cached(
                            "DELETE FROM liked_songs_deleted WHERE user_id = ?1 AND library_id = ?2",
                        )?
                        .execute([user, library])?;
                        (liked_songs::create(tx, user, library, now)?, 1)
                    }
                };
                if !positions(tx, library, id, track)?.is_empty() {
                    return Ok((None, unhidden));
                }
                let add = Op::Add { at: Some(0), items: vec![track] };
                Ok((Some(apply(tx, user, library, id, version, add, now)?), unhidden))
            })
            .await?;
        super::announce(&hub, user, change);
        if unhidden {
            hidden::announce(&hub, user, library, [track], false);
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `unlikeTrack`: takes the track out of the caller's Liked Songs. One not in it, or with no
/// Liked Songs to be in, is already not liked, which is the answer.
pub async fn unlike(
    State(db): State<Arc<Database>>,
    State(hub): State<Arc<Hub>>,
    Scope { user, library }: Scope,
    Path((_, Id(track))): Path<(Id, Id)>,
) -> Result<StatusCode, Problem> {
    super::settle(async move {
        let change = db.try_write(move |tx| take_out(tx, user, library, track)).await?;
        super::announce(&hub, user, change);
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Takes `track` out of `user`'s Liked Songs in `library`, in the writer, and returns the change
/// to announce: none when it is not in it, or they have none.
pub fn take_out(
    tx: &Connection,
    user: i64,
    library: i64,
    track: i64,
) -> Result<Option<Change>, Problem> {
    let Some((id, version)) = liked_songs::find(tx, user, library)? else { return Ok(None) };
    let positions = positions(tx, library, id, track)?;
    if positions.is_empty() {
        return Ok(None);
    }
    apply(tx, user, library, id, version, Op::Remove { positions }, now_ms()).map(Some)
}

/// Where `track` is in playlist `id` of `library`: each probe finds it through
/// `playlist_items_by_track`, then counts the items before it in key order.
fn positions(conn: &Connection, library: i64, id: i64, track: i64) -> rusqlite::Result<Vec<usize>> {
    conn.prepare_cached(
        "SELECT (SELECT count(*) FROM playlist_items j WHERE j.playlist_id = i.playlist_id \
         AND j.key < i.key) FROM playlist_items i INDEXED BY playlist_items_by_track \
         WHERE i.library_id = ?1 AND i.track_id = ?2 AND i.playlist_id = ?3",
    )?
    .query_map(params![library, track, id], |row| row.get(0))?
    .collect()
}

/// Applies `op` to `user`'s Liked Songs `id`, at `version`, in the writer: its items, totals,
/// version and modified time, as an edit does. Returns the change to announce.
fn apply(
    tx: &Connection,
    user: i64,
    library: i64,
    id: i64,
    version: i64,
    op: Op<i64>,
    now: i64,
) -> Result<Change, Problem> {
    let plan = edit::plan(tx, library, id, vec![op], now)?;
    edit::write(tx, user, library, id, version, &plan)?;
    let (count, duration) = plan.totals.unwrap_or_default();
    tx.prepare_cached(
        "UPDATE playlists SET track_count = ?2, duration_us = ?3, version = version + 1, \
         modified_at = ?4 WHERE id = ?1",
    )?
    .execute(params![id, count, duration, now])?;
    let liked = representation::get(tx, user, library, id)?.ok_or_else(Problem::not_found)?;
    Ok(Change::of(liked, plan.applied))
}

/// Whether playlist `id` is a Liked Songs.
pub(super) fn is_liked(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    conn.prepare_cached("SELECT kind = 'liked' FROM playlists WHERE id = ?1")?
        .query_row([id], |row| row.get(0))
}

/// `ops` as they apply to Liked Songs `id` of `library`, where adding a track likes it: an add
/// with no position goes to the top, as a like does, and adds only the tracks it does not hold,
/// each once, so each track is in it once. A track it holds as the edit is made is not added
/// again, even after a remove in the same edit.
pub(super) fn only_new(
    conn: &Connection,
    library: i64,
    id: i64,
    ops: Vec<Op<i64>>,
) -> rusqlite::Result<Vec<Op<i64>>> {
    let mut added = HashSet::new();
    let mut only_new = Vec::with_capacity(ops.len());
    for op in ops {
        only_new.push(match op {
            Op::Add { at, items } => {
                let mut new = Vec::new();
                for track in items {
                    if added.insert(track) && positions(conn, library, id, track)?.is_empty() {
                        new.push(track);
                    }
                }
                Op::Add { at: at.or(Some(0)), items: new }
            }
            op => op,
        });
    }
    Ok(only_new)
}
