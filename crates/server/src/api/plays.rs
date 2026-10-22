//! Recording plays: `recordPlays` (`api/openapi.yaml`, Listening).
//!
//! A play is reported when it starts, again as listen time accumulates, and when it ends, each
//! time under the `playId` its device chose, so it is recorded once however often it arrives
//! (`requirements/analytics.md` §1, §6). Reports can arrive out of order, as an offline device
//! catches up while a newer one is sent, so listen time never goes back and an end is kept.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use rusqlite::{Transaction, params};
use serde::Deserialize;
use serde_json::Value;

use super::authenticate::Caller;
use super::extract::Json;
use super::refs::credits_json;
use super::{Id, Problem};
use crate::db::plays::{End, Origin};
use crate::db::{self, Database, Text, now_ms};

#[derive(Deserialize)]
pub struct Plays {
    items: Vec<PlayReport>,
}

/// The spec's `PlayReport`. Its `deviceId` is not read: a play is the device's that reports it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayReport {
    play_id: String,
    library_id: String,
    track_id: Option<String>,
    external: Option<Value>,
    started_at: String,
    listen_time_ms: u32,
    end: Option<End>,
    context: Value,
    origin: Origin,
    reason: Option<Value>,
}

/// A report checked and ready to record.
struct Play {
    id: String,
    library: Option<Id>,
    track: Option<Id>,
    started_at: String,
    listen_time_ms: u32,
    end: Option<End>,
    context: Value,
    origin: Origin,
    reason: Option<Value>,
}

/// Records every report, or none if one is malformed. A play of a track that is not in a
/// library the caller reaches is left out: there is nothing to record it against, and saying
/// so would tell them what another library holds (`requirements/users.md` §10).
pub async fn record(
    State(db): State<Arc<Database>>,
    caller: Caller,
    Json(Plays { items }): Json<Plays>,
) -> Result<StatusCode, Problem> {
    let plays = items.into_iter().map(checked).collect::<Result<Vec<_>, _>>()?;
    let unparsed = db.write(move |tx| save(tx, caller, &plays)).await?;
    if let Some(at) = unparsed {
        return Err(Problem::invalid(format!("startedAt {at:?} is not a date and time.")));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `report` as a play to record, or the Problem that makes it malformed. A `null` reason is
/// none.
fn checked(report: PlayReport) -> Result<Play, Problem> {
    if !is_uuid(&report.play_id) {
        return Err(Problem::invalid("playId must be a UUID."));
    }
    if report.external.is_some() {
        return Err(Problem::invalid("Plays from outside a library are not recorded yet."));
    }
    let Some(track) = report.track_id else {
        return Err(Problem::invalid("A play needs a trackId."));
    };
    if !report.context.get("type").is_some_and(Value::is_string) {
        return Err(Problem::invalid("context must be an object with a type."));
    }
    Ok(Play {
        id: report.play_id.to_ascii_lowercase(),
        // An ID in any other spelling names nothing, and is left out like one out of reach.
        library: Id::canonical(&report.library_id),
        track: Id::canonical(&track),
        started_at: report.started_at,
        listen_time_ms: report.listen_time_ms,
        end: report.end,
        context: report.context,
        origin: report.origin,
        reason: report.reason.filter(|r| !r.is_null()),
    })
}

pub(super) fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Records `plays` for `caller`, unless one's `startedAt` is not a date and time: then it
/// records nothing and answers that. A `playId` names a play among the caller's own, so
/// another user's play under the same ID is a different play and never changes. Each play
/// keeps a snapshot of its track, which history shows and statistics count once the track or
/// its library is gone (`requirements/analytics.md` §8): its title, duration, release date,
/// artists, album and genres, with the keys the catalog matches them by.
fn save(tx: &Transaction, caller: Caller, plays: &[Play]) -> rusqlite::Result<Option<String>> {
    // SQLite reads the time, offset and all, and answers `NULL` for anything it cannot.
    let mut parse = tx.prepare_cached("SELECT CAST(unixepoch(?1, 'subsec') * 1000 AS INTEGER)")?;
    let mut started = Vec::with_capacity(plays.len());
    for play in plays {
        match parse.query_row([&play.started_at], |row| row.get::<_, Option<i64>>(0))? {
            Some(at) => started.push(at),
            None => return Ok(Some(play.started_at.clone())),
        }
    }
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
         WHERE t.id = ?5 AND t.library_id = ?4 AND (?13 OR EXISTS \
               (SELECT 1 FROM library_access WHERE user_id = ?2 AND library_id = ?4)) \
         ON CONFLICT (user_id, id) DO UPDATE SET \
             title = excluded.title, artists = excluded.artists, \
             artist_keys = excluded.artist_keys, album = excluded.album, \
             album_title_key = excluded.album_title_key, \
             album_artists_key = excluded.album_artists_key, genre_keys = excluded.genre_keys, \
             release_date = excluded.release_date, duration_us = excluded.duration_us, \
             listen_time_ms = max(listen_time_ms, excluded.listen_time_ms), \
             ended = coalesce(excluded.ended, ended), updated_at = excluded.updated_at \
         WHERE plays.track_id = excluded.track_id"
    ))?;
    let now = now_ms();
    for (play, started_at) in plays.iter().zip(started) {
        let (Some(library), Some(track)) = (play.library, play.track) else { continue };
        upsert.execute(params![
            play.id,
            caller.user,
            caller.device,
            library.0,
            track.0,
            started_at,
            play.listen_time_ms,
            play.end.map(Text),
            db::Json(&play.context),
            Text(play.origin),
            play.reason.as_ref().map(db::Json),
            now,
            caller.admin,
        ])?;
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
