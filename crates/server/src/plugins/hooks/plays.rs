//! The plays a `played` hook delivers: those each user who connected the plugin has ended,
//! from the ends in the feed of play events (`0005_activity.sql`).

use jewelcase_plugins::{Play, PlayEnd};
use rusqlite::{Connection, OptionalExtension, params};

use super::super::library::{TRACK_COLUMNS, track};
use super::BATCH;
use crate::db::Text;
use crate::db::plays::End;

/// The next batch of plays `user` ended in `library` after `position`, since they connected
/// `plugin`: the position after it, and the plays. One of a track since removed from the
/// library is passed over. A stored end the server does not know is an error.
pub fn plays(
    conn: &Connection,
    plugin: &str,
    library: i64,
    user: i64,
    position: i64,
) -> rusqlite::Result<(i64, Vec<Play>)> {
    let connected_at: Option<i64> = conn
        .prepare_cached(
            "SELECT connected_at FROM plugin_connections WHERE plugin_id = ?1 AND user_id = ?2",
        )?
        .query_row(params![plugin, user], |row| row.get(0))
        .optional()?;
    let Some(connected_at) = connected_at else { return Ok((position, Vec::new())) };
    let after = "e.library_id = ?1 AND e.user_id = ?2 AND e.kind = 'ended' AND e.seq > ?3 \
                 AND e.reported_at >= ?4";
    let last: Option<i64> = conn
        .prepare_cached(&format!(
            "SELECT max(seq) FROM (SELECT e.seq FROM play_events e WHERE {after} \
             ORDER BY e.seq LIMIT ?5)"
        ))?
        .query_row(params![library, user, position, connected_at, BATCH], |row| row.get(0))?;
    let Some(next) = last else { return Ok((position, Vec::new())) };
    let plays = conn
        .prepare_cached(&format!(
            "SELECT {}, p.started_at, p.listen_time_ms, p.ended FROM play_events e \
             JOIN plays p ON p.user_id = e.user_id AND p.id = e.play_id \
             JOIN tracks t ON t.id = p.track_id AND t.library_id = e.library_id \
             JOIN albums al ON al.id = t.album_id \
             WHERE {after} AND e.seq <= ?5 ORDER BY e.seq",
            TRACK_COLUMNS.join(", ")
        ))?
        .query_map(params![library, user, position, connected_at, next], |row| {
            let Text(end) = row.get::<_, Text<End>>(15)?;
            Ok(Play {
                track: track(row)?,
                started_at_ms: row.get(13)?,
                listen_time_ms: row.get(14)?,
                end: end.into(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok((next, plays))
}

impl From<End> for PlayEnd {
    /// The end a plugin is told of: the one the play was reported with.
    fn from(end: End) -> PlayEnd {
        match end {
            End::Finished => PlayEnd::Finished,
            End::Skipped => PlayEnd::Skipped,
            End::Stopped => PlayEnd::Stopped,
        }
    }
}
