//! `likeTrack` and `unlikeTrack` (`api/openapi.yaml`): a like puts a track at the top of the
//! caller's Liked Songs, made again if they deleted it, and an unlike takes it out
//! (`requirements/playlists.md` §11). Neither is checked against a version: each says whether a
//! track is liked, not where it sits, so it applies whatever else changed the playlist. Nor
//! does either read the playlist: each finds the track's rows through `playlist_items_by_track`,
//! writes only those, and moves the totals by them, so the writer is held as briefly in a Liked
//! Songs of thousands as in an empty one. Each is announced to the caller's devices as an edit
//! is. Liking a hidden track unhides it, and hiding one unlikes it ([`take_out`]), so the two
//! never contradict each other (`requirements/conventions.md` §8). And how an edit's adds apply
//! to a Liked Songs ([`only_new`]).

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use jewelcase_core::playlist::Op;
use rusqlite::{Connection, OptionalExtension, params};

use super::announce::{AppliedOp, Change};
use super::{KEY_GAP, edit, representation};
use crate::api::extract::Path;
use crate::api::hidden;
use crate::api::realtime::{Hub, ServerMessage};
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
                let duration = duration(tx, library, track)?.ok_or_else(Problem::not_found)?;
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
                if holds(tx, library, id, track)? {
                    return Ok::<_, Problem>((None, unhidden));
                }
                put_first(tx, user, library, (id, version), track, now)?;
                let added =
                    AppliedOp::Add { at: Some(0), track_ids: vec![Id(track)], added_at: now };
                let change = changed(tx, user, library, id, (1, duration), now, added)?;
                Ok((Some(change), unhidden))
            })
            .await?;
        if change.is_some() {
            announce(&hub, user, library, [track], true);
        }
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
        if change.is_some() {
            announce(&hub, user, library, [track], false);
        }
        super::announce(&hub, user, change);
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Tells `user`'s devices that `tracks` of `library` were liked, or unliked, so each shows it on
/// every copy of them it holds, which a change to Liked Songs alone, naming a removed item by its
/// position, would not let it do. Call it once the change is committed.
pub fn announce(
    hub: &Hub,
    user: i64,
    library: i64,
    tracks: impl IntoIterator<Item = i64>,
    liked: bool,
) {
    for track in tracks {
        let message =
            ServerMessage::TrackLiked { library_id: Id(library), track_id: Id(track), liked };
        hub.broadcast(user, &message);
    }
}

/// Takes `track` out of `user`'s Liked Songs in `library`, in the writer, and returns the change
/// to announce: none when it is not in it, or they have none.
pub fn take_out(
    tx: &Connection,
    user: i64,
    library: i64,
    track: i64,
) -> Result<Option<Change>, Problem> {
    let Some((id, _)) = liked_songs::find(tx, user, library)? else { return Ok(None) };
    let mut positions = positions(tx, library, id, track)?;
    if positions.is_empty() {
        return Ok(None);
    }
    positions.sort_unstable();
    tx.prepare_cached(
        "DELETE FROM playlist_items INDEXED BY playlist_items_by_track \
         WHERE library_id = ?1 AND track_id = ?2 AND playlist_id = ?3",
    )?
    .execute(params![library, track, id])?;
    // Retention keeps a track any playlist holds, so it is still there to subtract.
    let duration = duration(tx, library, track)?.unwrap_or(0);
    let count = i64::try_from(positions.len()).unwrap_or(i64::MAX);
    let removed = AppliedOp::Remove { positions };
    let totals = (-count, -count.saturating_mul(duration));
    changed(tx, user, library, id, totals, now_ms(), removed).map(Some)
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

/// Whether playlist `id` of `library` holds `track`, found through `playlist_items_by_track`.
fn holds(conn: &Connection, library: i64, id: i64, track: i64) -> rusqlite::Result<bool> {
    conn.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM playlist_items INDEXED BY playlist_items_by_track \
         WHERE library_id = ?1 AND track_id = ?2 AND playlist_id = ?3)",
    )?
    .query_row(params![library, track, id], |row| row.get(0))
}

/// The duration of `track`, or `None` if `library` has no such track.
fn duration(conn: &Connection, library: i64, track: i64) -> rusqlite::Result<Option<i64>> {
    conn.prepare_cached("SELECT duration_us FROM tracks WHERE library_id = ?1 AND id = ?2")?
        .query_row([library, track], |row| row.get(0))
        .optional()
}

/// Puts `track` at the top of `user`'s Liked Songs `id`, at `version`, under a key [`KEY_GAP`]
/// before the first: one row written, and none read but the first. Only when no key is left
/// before the first is the playlist renumbered, as an edit would.
fn put_first(
    tx: &Connection,
    user: i64,
    library: i64,
    (id, version): (i64, i64),
    track: i64,
    now: i64,
) -> Result<(), Problem> {
    let first: Option<i64> = tx
        .prepare_cached("SELECT min(key) FROM playlist_items WHERE playlist_id = ?1")?
        .query_row([id], |row| row.get(0))?;
    let Some(key) = first.map_or(Some(KEY_GAP), |first| first.checked_sub(KEY_GAP)) else {
        let add = Op::Add { at: Some(0), items: vec![track] };
        let plan = edit::plan(tx, library, id, vec![add], now)?;
        return edit::write(tx, user, library, id, version, &plan);
    };
    tx.prepare_cached(
        "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![id, key, library, track, now])?;
    Ok(())
}

/// Moves `user`'s Liked Songs `id` on a version once `op` has changed its items, adding
/// `(count, duration)` to its totals. Returns the change to announce.
fn changed(
    tx: &Connection,
    user: i64,
    library: i64,
    id: i64,
    (count, duration): (i64, i64),
    now: i64,
    op: AppliedOp,
) -> Result<Change, Problem> {
    tx.prepare_cached(
        "UPDATE playlists SET track_count = max(track_count + ?2, 0), \
         duration_us = max(duration_us + ?3, 0), version = version + 1, modified_at = ?4 \
         WHERE id = ?1",
    )?
    .execute(params![id, count, duration, now])?;
    let liked = representation::get(tx, user, library, id)?.ok_or_else(Problem::not_found)?;
    Ok(Change::of(liked, vec![op]))
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
                    if added.insert(track) && !holds(conn, library, id, track)? {
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
