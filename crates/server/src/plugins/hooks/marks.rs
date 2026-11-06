//! The likes and hides a `marked` hook delivers: each like, unlike, hide, and unhide a user who
//! connected the plugin made since, from the feed of marks (`0005_activity.sql`,
//! `design/hooks.md` §1.4).

use jewelcase_plugins::{Mark, MarkKind};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use rusqlite::{Connection, OptionalExtension, params};

use super::super::library::{TRACK_COLUMNS, track};
use super::BATCH;

/// The next batch of marks `user` made in `library` after `position`, since they connected
/// `plugin`: the position after it, and the marks. One of a track since removed from the library
/// is passed over, since there is no track to send.
pub fn marks(
    conn: &Connection,
    plugin: &str,
    library: i64,
    user: i64,
    position: i64,
) -> rusqlite::Result<(i64, Vec<Mark>)> {
    let connected_at: Option<i64> = conn
        .prepare_cached(
            "SELECT connected_at FROM plugin_connections WHERE plugin_id = ?1 AND user_id = ?2",
        )?
        .query_row(params![plugin, user], |row| row.get(0))
        .optional()?;
    let Some(connected_at) = connected_at else { return Ok((position, Vec::new())) };
    let after = "e.library_id = ?1 AND e.user_id = ?2 AND e.seq > ?3 AND e.created_at >= ?4";
    let last: Option<i64> = conn
        .prepare_cached(&format!(
            "SELECT max(seq) FROM (SELECT e.seq FROM mark_events e WHERE {after} \
             ORDER BY e.seq LIMIT ?5)"
        ))?
        .query_row(params![library, user, position, connected_at, BATCH], |row| row.get(0))?;
    let Some(next) = last else { return Ok((position, Vec::new())) };
    let marks = conn
        .prepare_cached(&format!(
            "SELECT {}, e.kind, e.created_at FROM mark_events e \
             JOIN tracks t ON t.id = e.track_id AND t.library_id = e.library_id \
             JOIN albums al ON al.id = t.album_id \
             WHERE {after} AND e.seq <= ?5 ORDER BY e.seq",
            TRACK_COLUMNS.join(", ")
        ))?
        .query_map(params![library, user, position, connected_at, next], |row| {
            let Kind(kind) = row.get(13)?;
            Ok(Mark { track: track(row)?, kind, at_ms: row.get(14)? })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok((next, marks))
}

/// A mark's kind as `mark_events.kind` stores it.
struct Kind(MarkKind);

impl FromSql for Kind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "liked" => Ok(Kind(MarkKind::Liked)),
            "unliked" => Ok(Kind(MarkKind::Unliked)),
            "hidden" => Ok(Kind(MarkKind::Hidden)),
            "unhidden" => Ok(Kind(MarkKind::Unhidden)),
            other => Err(FromSqlError::Other(format!("no such mark: {other}").into())),
        }
    }
}
