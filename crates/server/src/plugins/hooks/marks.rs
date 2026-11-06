//! The likes and hides a `marked` hook delivers to each user who connected the plugin
//! (`design/hooks.md` §1.4): first the ones they have, a page at a time by track, then each like,
//! unlike, hide, and unhide since, from the feed of marks (`0005_activity.sql`).
//!
//! The first page notes the feed's head in the same read, and the feed is followed from there,
//! so a change made while the pages go out is never missed: it may come both in a page and from
//! the feed, which is harmless, since each mark says what the song now is.

use jewelcase_plugins::{Mark, MarkKind};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::super::library::{TRACK_COLUMNS, track};
use super::{BATCH, Due, Next};

/// What `due`'s plugin is next to be told of `user`'s marks in its library: where it is once
/// that is handled, and the marks. While their likes and hides as they stand go out
/// ([`Due::after`]) it is the next page of them; once they are out, the feed after the
/// position. A mark of a track since removed from the library is passed over, since there is no
/// track to send. Nothing is sent for a user who is not connected.
pub fn marks(conn: &Connection, due: &Due, user: i64) -> rusqlite::Result<(Next, Vec<Mark>)> {
    let connected: bool = conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM plugin_connections \
             WHERE plugin_id = ?1 AND user_id = ?2)",
        )?
        .query_row(params![due.plugin, user], |row| row.get(0))?;
    let here = Next { position: due.position, after: due.after };
    if !connected {
        return Ok((here, Vec::new()));
    }
    match due.after {
        Some(after) => standing(conn, due, user, after),
        None => feed(conn, due.library, user, due.position),
    }
}

/// The next page of `user`'s likes and hides in `due`'s library as they stand, after track
/// `after`, by track. The first, after 0, notes the feed's head; once a page is short, all are
/// out and the feed is followed from that head.
fn standing(
    conn: &Connection,
    due: &Due,
    user: i64,
    after: i64,
) -> rusqlite::Result<(Next, Vec<Mark>)> {
    let head = if after == 0 {
        conn.prepare_cached("SELECT coalesce(max(seq), 0) FROM mark_events")?
            .query_row([], |row| row.get(0))?
    } else {
        due.position
    };
    // Liking and hiding undo each other, so a track is one or the other, never both.
    let marks: Vec<(i64, Mark)> = conn
        .prepare_cached(&format!(
            "SELECT {}, m.kind, m.at FROM ( \
               SELECT i.track_id, 'liked' AS kind, i.added_at AS at FROM playlists p \
               JOIN playlist_items i ON i.playlist_id = p.id \
               WHERE p.user_id = ?2 AND p.library_id = ?1 AND p.kind = 'liked' \
                 AND i.track_id > ?3 \
               UNION ALL \
               SELECT h.track_id, 'hidden', h.hidden_at FROM hidden_tracks h \
               WHERE h.user_id = ?2 AND h.library_id = ?1 AND h.track_id > ?3) m \
             JOIN tracks t ON t.id = m.track_id AND t.library_id = ?1 \
             JOIN albums al ON al.id = t.album_id \
             ORDER BY m.track_id LIMIT ?4",
            TRACK_COLUMNS.join(", ")
        ))?
        .query_map(params![due.library, user, after, BATCH], |row| Ok((row.get(0)?, mark(row)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let full = i64::try_from(marks.len()).is_ok_and(|n| n == BATCH);
    let after = if full { marks.last().map(|(track, _)| *track) } else { None };
    Ok((Next { position: head, after }, marks.into_iter().map(|(_, mark)| mark).collect()))
}

/// The next batch of marks `user` made in `library` after `position`, from the feed.
fn feed(
    conn: &Connection,
    library: i64,
    user: i64,
    position: i64,
) -> rusqlite::Result<(Next, Vec<Mark>)> {
    let after = "e.library_id = ?1 AND e.user_id = ?2 AND e.seq > ?3";
    let last: Option<i64> = conn
        .prepare_cached(&format!(
            "SELECT max(seq) FROM (SELECT e.seq FROM mark_events e WHERE {after} \
             ORDER BY e.seq LIMIT ?4)"
        ))?
        .query_row(params![library, user, position, BATCH], |row| row.get(0))
        .optional()?
        .flatten();
    let Some(next) = last else { return Ok((position.into(), Vec::new())) };
    let marks = conn
        .prepare_cached(&format!(
            "SELECT {}, e.kind, e.created_at FROM mark_events e \
             JOIN tracks t ON t.id = e.track_id AND t.library_id = e.library_id \
             JOIN albums al ON al.id = t.album_id \
             WHERE {after} AND e.seq <= ?4 ORDER BY e.seq",
            TRACK_COLUMNS.join(", ")
        ))?
        .query_map(params![library, user, position, next], mark)?
        .collect::<rusqlite::Result<_>>()?;
    Ok((next.into(), marks))
}

/// A mark from a row of [`TRACK_COLUMNS`], then its kind and when.
fn mark(row: &Row) -> rusqlite::Result<Mark> {
    let Kind(kind) = row.get(13)?;
    Ok(Mark { track: track(row)?, kind, at_ms: row.get(14)? })
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
