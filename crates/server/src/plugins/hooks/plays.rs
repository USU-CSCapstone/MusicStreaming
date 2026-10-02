//! The plays a `played` hook delivers: those each user who connected the plugin has ended,
//! from the feed of play ends (`0013_plugin_played.sql`).

use jewelcase_plugins::{Play, PlayEnd};
use rusqlite::{Connection, OptionalExtension, params};

use super::super::library::{TRACK_COLUMNS, track};
use super::BATCH;

/// The next batch of plays `user` ended in `library` after `position`, since they connected
/// `plugin`: the position after it, and the plays. One of a track since removed from the
/// library is passed over.
pub fn plays(
    conn: &Connection,
    plugin: &str,
    library: i64,
    user: i64,
    position: i64,
) -> rusqlite::Result<(i64, Vec<Play>)> {
    let connected_at: Option<i64> = conn
        .prepare_cached(
            "SELECT connected_at FROM plugin_user_settings WHERE plugin_id = ?1 AND user_id = ?2",
        )?
        .query_row(params![plugin, user], |row| row.get(0))
        .optional()?;
    let Some(connected_at) = connected_at else { return Ok((position, Vec::new())) };
    let after = "e.library_id = ?1 AND e.user_id = ?2 AND e.seq > ?3 AND e.at >= ?4";
    let last: Option<i64> = conn
        .prepare_cached(&format!(
            "SELECT max(seq) FROM (SELECT e.seq FROM play_ends e WHERE {after} \
             ORDER BY e.seq LIMIT ?5)"
        ))?
        .query_row(params![library, user, position, connected_at, BATCH], |row| row.get(0))?;
    let Some(next) = last else { return Ok((position, Vec::new())) };
    let plays = conn
        .prepare_cached(&format!(
            "{TRACK_COLUMNS}, p.started_at, p.listen_time_ms, p.ended FROM play_ends e \
             JOIN plays p ON p.id = e.play_id JOIN tracks t ON t.id = p.track_id \
             JOIN albums al ON al.id = t.album_id WHERE {after} AND e.seq <= ?5 ORDER BY e.seq"
        ))?
        .query_map(params![library, user, position, connected_at, next], |row| {
            let end = match row.get_ref(15)?.as_str()? {
                "finished" => PlayEnd::Finished,
                "skipped" => PlayEnd::Skipped,
                _ => PlayEnd::Stopped,
            };
            Ok(Play {
                track: track(row)?,
                started_at: row.get::<_, i64>(13)? as u64,
                listen_time_ms: row.get::<_, i64>(14)? as u64,
                end,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok((next, plays))
}
