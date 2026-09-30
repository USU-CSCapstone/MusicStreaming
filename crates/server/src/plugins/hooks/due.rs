//! Which hooks are due, and the event each delivers.
//!
//! A hook is due for a plugin in a library when the plugin is enabled there, the hook (and the
//! permission its events need) is granted, it is not waiting to retry a failure, and there is
//! something past its position: changes, a finished scan, or its interval gone by.

use jewelcase_plugins::{Event, ScanFinished, TracksChanged};
use rusqlite::{Connection, OptionalExtension, params};

use super::{BATCH, Due, Hook};

/// The pair's position in the hook `?2`, and its failures.
const POSITION: &str = "coalesce(c.position, 0), coalesce(c.failures, 0) FROM plugin_libraries p \
     LEFT JOIN plugin_cursors c ON c.plugin_id = p.plugin_id AND c.library_id = p.library_id \
          AND c.hook = ?2";

/// Granted in the library, by the pair's plugin.
fn granted_here(permission: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM plugin_library_grants g WHERE g.plugin_id = p.plugin_id \
         AND g.library_id = p.library_id AND g.permission = '{permission}')"
    )
}

const READY: &str = "p.enabled AND (c.retry_at IS NULL OR c.retry_at <= ?1)";

pub fn all(conn: &Connection, now: i64) -> rusqlite::Result<Vec<Due>> {
    let tracks = format!(
        "SELECT p.plugin_id, p.library_id, {POSITION} WHERE {READY} AND {} AND {} \
         AND EXISTS (SELECT 1 FROM library_changes f \
                     WHERE f.library_id = p.library_id AND f.seq > coalesce(c.position, 0))",
        granted_here("tracksChanged"),
        granted_here("libraryRead"),
    );
    // Only scans finished before now, so one finishing this millisecond is never skipped.
    let scans = format!(
        "SELECT p.plugin_id, p.library_id, {POSITION} WHERE {READY} AND {} AND {} \
         AND EXISTS (SELECT 1 FROM scans s WHERE s.library_id = p.library_id \
                     AND s.state = 'completed' AND s.finished_at > coalesce(c.position, 0) \
                     AND s.finished_at < ?1)",
        granted_here("scanFinished"),
        granted_here("libraryRead"),
    );
    // At once if it has never run; after that at the interval from its manifest.
    let schedules = format!(
        "SELECT p.plugin_id, p.library_id, {POSITION} WHERE {READY} \
         AND EXISTS (SELECT 1 FROM plugin_grants g WHERE g.plugin_id = p.plugin_id \
                     AND g.permission = 'schedule') \
         AND (c.position IS NULL OR c.position + 60000 * \
              (SELECT json_extract(r.value, '$.everyMinutes') \
               FROM plugins pl, json_each(pl.manifest, '$.permissions') r \
               WHERE pl.id = p.plugin_id AND json_extract(r.value, '$.permission') = 'schedule') <= ?1)"
    );
    let mut due = Vec::new();
    for (hook, sql) in
        [(Hook::TracksChanged, tracks), (Hook::ScanFinished, scans), (Hook::Schedule, schedules)]
    {
        let mut statement = conn.prepare_cached(&sql)?;
        let pairs = statement.query_map(params![now, hook.name()], |row| {
            Ok(Due {
                hook,
                plugin: row.get(0)?,
                library: row.get(1)?,
                position: row.get(2)?,
                failures: row.get(3)?,
            })
        })?;
        due.extend(pairs.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    Ok(due)
}

/// What `due` delivers: the position once it is handled, and its event. No event means
/// nothing for the plugin, and the position moves on anyway.
pub fn event(conn: &Connection, due: &Due, now: i64) -> rusqlite::Result<(i64, Option<Event>)> {
    Ok(match due.hook {
        Hook::TracksChanged => {
            let (next, changes) = batch(conn, due.library, due.position)?;
            let empty = changes.changed.is_empty() && changes.removed.is_empty();
            (next, (!empty).then_some(Event::TracksChanged(changes)))
        }
        Hook::ScanFinished => match scans(conn, due.library, due.position, now)? {
            Some((next, totals)) => (next, Some(Event::ScanFinished(totals))),
            None => (due.position, None),
        },
        Hook::Schedule => (now, Some(Event::Scheduled)),
    })
}

/// The next batch of changes to `library` after `position`: the position after it, and the
/// tracks it names.
pub fn batch(
    conn: &Connection,
    library: i64,
    position: i64,
) -> rusqlite::Result<(i64, TracksChanged)> {
    let rows: Vec<(i64, String, i64, String)> = conn
        .prepare_cached(
            "SELECT seq, entity_type, entity_id, op FROM library_changes \
             WHERE library_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?
        .query_map(params![library, position, BATCH], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let next = rows.last().map_or(position, |(seq, ..)| *seq);
    let mut changes = TracksChanged { changed: Vec::new(), removed: Vec::new() };
    for (_, entity, id, op) in rows {
        match (entity.as_str(), op.as_str()) {
            ("track", "delete") => changes.removed.push(id as u64),
            ("track", _) => changes.changed.push(id as u64),
            _ => {}
        }
    }
    Ok((next, changes))
}

/// The scans of `library` completed after `position` and before now, added together, and
/// when the last of them finished.
fn scans(
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
