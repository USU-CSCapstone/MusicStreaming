//! What `tracksChanged`, `albumsChanged`, `artistsChanged`, and `scanFinished` deliver: changes
//! from the library change feed (`0004_feeds.sql`), and finished scans.

use jewelcase_plugins::{Changes, Event, ScanFinished};
use rusqlite::{Connection, OptionalExtension, params};

use super::{BATCH, Due};

/// The next batch of changes to `due`'s library's entities of type `entity`, as the event
/// `event` makes of them: the position after it, and the event, if there are any.
pub fn changed(
    conn: &Connection,
    due: &Due,
    entity: &str,
    event: fn(Changes) -> Event,
) -> rusqlite::Result<(i64, Option<Event>)> {
    let (next, changes) = batch(conn, due.library, due.position, entity)?;
    Ok((next, changes.map(event)))
}

/// The next batch of changes to `library`'s entities of type `entity` (`track`, `album`, or
/// `artist`) after `position`: the position after it, and the IDs it names, if there are any.
pub fn batch(
    conn: &Connection,
    library: i64,
    position: i64,
    entity: &str,
) -> rusqlite::Result<(i64, Option<Changes>)> {
    let rows: Vec<(i64, u64, String)> = conn
        .prepare_cached(
            "SELECT seq, entity_id, op FROM library_changes \
             WHERE library_id = ?1 AND entity_type = ?2 AND seq > ?3 ORDER BY seq LIMIT ?4",
        )?
        .query_map(params![library, entity, position, BATCH], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let Some(&(next, ..)) = rows.last() else { return Ok((position, None)) };
    let mut changes = Changes { changed: Vec::new(), removed: Vec::new() };
    for (_, id, op) in rows {
        match op.as_str() {
            "delete" => changes.removed.push(id),
            _ => changes.changed.push(id),
        }
    }
    Ok((next, Some(changes)))
}

/// The scans of `library` completed after `position` and before now, added together, and
/// when the last of them finished.
pub fn scans(
    conn: &Connection,
    library: i64,
    position: i64,
    now: i64,
) -> rusqlite::Result<Option<(i64, ScanFinished)>> {
    conn.prepare_cached(
        "SELECT max(finished_at), sum(files_seen), sum(added), sum(updated), sum(moved), \
                sum(missing), sum(problems) FROM scans \
         WHERE library_id = ?1 AND state = 'completed' AND finished_at > ?2 AND finished_at < ?3 \
         HAVING count(*) > 0",
    )?
    .query_row(params![library, position, now], |row| {
        Ok((
            row.get(0)?,
            ScanFinished {
                files_seen: row.get(1)?,
                added: row.get(2)?,
                updated: row.get(3)?,
                moved: row.get(4)?,
                missing: row.get(5)?,
                problems: row.get(6)?,
            },
        ))
    })
    .optional()
}
