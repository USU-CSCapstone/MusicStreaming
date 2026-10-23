//! Recording plays: `recordPlays` (`api/openapi.yaml`, Listening).
//!
//! A play is reported when it starts, again as listen time accumulates, and when it ends, each
//! time under the `playId` its device chose, so it is recorded once however often it arrives
//! (`requirements/analytics.md` §1, §6). Reports can arrive out of order, as an offline device
//! catches up while a newer one is sent, so listen time never goes back and an end is kept.

mod context;

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use rusqlite::{Transaction, params};
use serde::Deserialize;
use serde::de::IgnoredAny;

use context::{Context, Reason};

use super::extract;
use super::refs::credits_json;
use super::scope::Caller;
use super::time::Timestamp;
use super::{AnyId, Problem, Uuid};
use crate::db::plays::{End, Origin};
use crate::db::{self, Database, Text, now_ms, reaches};

/// The spec's `recordPlays` body.
#[derive(Deserialize)]
pub struct Plays {
    items: Vec<PlayReport>,
}

/// The spec's `PlayReport`. Its `deviceId` is not read: a play is the device's that reports it.
/// Of `external` only its presence is read, since a play of an external track is not recorded.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayReport {
    play_id: Uuid,
    library_id: AnyId,
    track_id: Option<AnyId>,
    external: Option<IgnoredAny>,
    started_at: Timestamp,
    listen_time_ms: u32,
    end: Option<End>,
    context: Context,
    origin: Origin,
    reason: Option<Reason>,
}

/// A report checked and ready to record.
struct Play {
    id: Uuid,
    /// A library or track the report names badly is none, and the play is left out like one
    /// out of reach.
    library: AnyId,
    track: AnyId,
    /// Milliseconds after the Unix epoch, no later than when the report was heard.
    started_at: i64,
    listen_time_ms: u32,
    end: Option<End>,
    context: Context,
    origin: Origin,
    reason: Option<Reason>,
}

/// Records every report, or none if one is malformed. A play of a track that is not in a
/// library the caller reaches is left out: there is nothing to record it against, and saying
/// so would tell them what another library holds (`requirements/users.md` §10). A play of an
/// external track is left out too, as plays outside a library are not recorded.
pub async fn record(
    State(db): State<Arc<Database>>,
    caller: Caller,
    extract::Json(Plays { items }): extract::Json<Plays>,
) -> Result<StatusCode, Problem> {
    let now = now_ms();
    let plays = items
        .into_iter()
        .filter_map(|report| checked(report, now).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    db.write(move |tx| save(tx, caller, &plays, now)).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `report` as a play to record, `None` for an external track's, or the Problem that makes it
/// malformed. A `startedAt` after `now` is taken as `now`: a device's clock can run ahead, and
/// the play is still the user's. One before 1970 is taken as 1970, where stored times begin.
fn checked(report: PlayReport, now: i64) -> Result<Option<Play>, Problem> {
    let track = match (report.track_id, report.external) {
        (Some(track), None) => track,
        (None, Some(_)) => return Ok(None),
        _ => return Err(Problem::invalid("A play names exactly one of trackId and external.")),
    };
    Ok(Some(Play {
        id: report.play_id,
        library: report.library_id,
        track,
        started_at: report.started_at.0.clamp(0, now),
        listen_time_ms: report.listen_time_ms,
        end: report.end,
        context: report.context,
        origin: report.origin,
        reason: report.reason,
    }))
}

/// Records `plays` for `caller`, as heard at `now`. A `playId` names a play among the
/// caller's own, so another user's play under the same ID is a different play and never
/// changes. Each play keeps a snapshot of its track, which history shows and statistics count
/// once the track or its library is gone (`requirements/analytics.md` §8): its title,
/// duration, release date, artists, album and genres, with the keys the catalog matches them
/// by.
fn save(tx: &Transaction, caller: Caller, plays: &[Play], now: i64) -> rusqlite::Result<()> {
    // Each report refreshes the snapshot, so a play reported again after a retag takes the
    // new tags; once the track or its library is gone no report finds it, and the play keeps
    // the last ones. A report naming a track other than the play's changes nothing of it.
    // Track IDs are unique across libraries, so that check covers the library too.
    let mut upsert = tx.prepare_cached(concat!(
        "INSERT INTO plays (id, user_id, device_id, library_id, track_id, title, artists, \
                            artist_keys, album, album_title_key, album_artists_key, genre_keys, \
                            release_date, duration_us, started_at, listen_time_ms, ended, \
                            context, origin, reason, updated_at) \
         SELECT ?1, ?2, ?3, t.library_id, t.id, t.title, ",
        credits_json!("credited.name", "track", "t.id"),
        ", ",
        credits_json!("credited.name_key", "track", "t.id"),
        ", al.title, al.title_key, al.artists_key, \
           (SELECT json_group_array(tag.name_key ORDER BY tag.name_key) FROM track_tags tagged \
            JOIN tags tag ON tag.id = tagged.tag_id \
            WHERE tagged.track_id = t.id AND tag.type = 'genre'), \
           t.release_date, t.duration_us, ?6, ?7, ?8, ?9, ?10, ?11, ?12 \
         FROM tracks t JOIN albums al ON al.id = t.album_id \
         WHERE t.id = ?5 AND t.library_id = ?4 AND ",
        reaches!(admin: "?13", user: "?2", library: "?4"),
        " ON CONFLICT (user_id, id) DO UPDATE SET \
             title = excluded.title, artists = excluded.artists, \
             artist_keys = excluded.artist_keys, album = excluded.album, \
             album_title_key = excluded.album_title_key, \
             album_artists_key = excluded.album_artists_key, genre_keys = excluded.genre_keys, \
             release_date = excluded.release_date, duration_us = excluded.duration_us, \
             listen_time_ms = max(listen_time_ms, excluded.listen_time_ms), \
             ended = coalesce(excluded.ended, ended), updated_at = excluded.updated_at \
         WHERE plays.track_id = excluded.track_id"
    ))?;
    for play in plays {
        upsert.execute(params![
            play.id.as_str(),
            caller.user,
            caller.device,
            play.library,
            play.track,
            play.started_at,
            play.listen_time_ms,
            play.end.map(Text),
            db::Json(&play.context),
            Text(play.origin),
            play.reason.as_ref().map(db::Json),
            now,
            caller.admin,
        ])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
