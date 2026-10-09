//! Which hooks are due, and the event each delivers.
//!
//! A hook is due for a plugin in a library when the plugin is enabled there, the hook (and the
//! permission its events need) is granted, it is not waiting to retry a failure, and there is
//! something past its position: changes of the kind it asked for, a finished scan, its interval gone by, plays ended
//! by a user who connected it, a play one of them has just started and is still playing, or
//! searches by a user who shares them with it. Each user has a position of their own in
//! `played`, `playing`, and `searched`.

use jewelcase_plugins::{AlbumsChanged, ArtistsChanged, Event, ScanFinished, TracksChanged};
use rusqlite::{Connection, OptionalExtension, params};

use super::playing::{deliverable, playing};
use super::plays::plays;
use super::searches::searches;
use super::{BATCH, Due, Hook};

/// The pair's position in the hook `?2`, its failures, and the user it is for. `per_user`
/// pairs it with each user who connected the plugin, as `s`; otherwise the user is 0.
fn position(per_user: bool) -> String {
    let (users, user) = match per_user {
        true => ("JOIN plugin_user_settings s ON s.plugin_id = p.plugin_id", "s.user_id"),
        false => ("", "0"),
    };
    format!(
        "coalesce(c.position, 0), coalesce(c.failures, 0), {user} FROM plugin_libraries p {users} \
         LEFT JOIN plugin_cursors c ON c.plugin_id = p.plugin_id \
              AND c.library_id = p.library_id AND c.hook = ?2 AND c.user_id = {user}"
    )
}

/// Granted to the pair's plugin itself.
fn granted_to_plugin(permission: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM plugin_grants g WHERE g.plugin_id = p.plugin_id \
         AND g.permission = '{permission}')"
    )
}

/// Granted in the library, by the pair's plugin.
fn granted_here(permission: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM plugin_library_grants g WHERE g.plugin_id = p.plugin_id \
         AND g.library_id = p.library_id AND g.permission = '{permission}')"
    )
}

const READY: &str = "p.enabled AND (c.retry_at IS NULL OR c.retry_at <= ?1)";

pub fn all(conn: &Connection, now: i64) -> rusqlite::Result<Vec<Due>> {
    let pair = position(false);
    // Changes of one kind of entity in the library, for the hook that asked for them.
    let changes = |hook: &str, entity: &str| {
        format!(
            "SELECT p.plugin_id, p.library_id, {pair} WHERE {READY} AND {} AND {} \
             AND EXISTS (SELECT 1 FROM library_changes f \
                         WHERE f.library_id = p.library_id AND f.entity_type = '{entity}' \
                         AND f.seq > coalesce(c.position, 0))",
            granted_here(hook),
            granted_here("libraryRead"),
        )
    };
    // Only scans finished before now, so one finishing this millisecond is never skipped.
    let scans = format!(
        "SELECT p.plugin_id, p.library_id, {pair} WHERE {READY} AND {} AND {} \
         AND EXISTS (SELECT 1 FROM scans s WHERE s.library_id = p.library_id \
                     AND s.state = 'completed' AND s.finished_at > coalesce(c.position, 0) \
                     AND s.finished_at < ?1)",
        granted_here("scanFinished"),
        granted_here("libraryRead"),
    );
    // At once if it has never run; after that at the interval from its manifest.
    let schedules = format!(
        "SELECT p.plugin_id, p.library_id, {pair} WHERE {READY} AND {} \
         AND (c.position IS NULL OR c.position + 60000 * \
              (SELECT json_extract(r.value, '$.everyMinutes') \
               FROM plugins pl, json_each(pl.manifest, '$.permissions') r \
               WHERE pl.id = p.plugin_id AND json_extract(r.value, '$.permission') = 'schedule') <= ?1)",
        granted_to_plugin("schedule"),
    );
    // For each user who connected it, plays they ended in the library since they did.
    let played = format!(
        "SELECT p.plugin_id, p.library_id, {} WHERE {READY} AND {} AND {} AND {} \
         AND EXISTS (SELECT 1 FROM play_ends e WHERE e.library_id = p.library_id \
                     AND e.user_id = s.user_id AND e.seq > coalesce(c.position, 0) \
                     AND e.at >= s.connected_at)",
        position(true),
        granted_to_plugin("played"),
        granted_to_plugin("listeningActivity"),
        granted_here("libraryRead"),
    );
    // For each user who connected it, a play they started in the library that is still fresh.
    let playing = format!(
        "SELECT p.plugin_id, p.library_id, {} WHERE {READY} AND {} AND {} AND {} \
         AND EXISTS (SELECT 1 FROM play_starts st JOIN plays pl ON pl.id = st.play_id WHERE {})",
        position(true),
        granted_to_plugin("playing"),
        granted_to_plugin("listeningActivity"),
        granted_here("libraryRead"),
        deliverable("p.library_id", "s.user_id", "coalesce(c.position, 0)"),
    );
    // For each user who shares their searches with it, what they searched or removed since; and
    // for each who stopped, the request to forget, which needs only the hook.
    let searched = format!(
        "SELECT p.plugin_id, p.library_id, coalesce(c.position, 0), coalesce(c.failures, 0), \
                u.user_id FROM plugin_libraries p \
         JOIN (SELECT plugin_id, user_id FROM plugin_search_sharing \
               UNION SELECT plugin_id, user_id FROM search_events WHERE kind = 'stopped') u \
              ON u.plugin_id = p.plugin_id \
         LEFT JOIN plugin_search_sharing ss \
              ON ss.plugin_id = p.plugin_id AND ss.user_id = u.user_id \
         LEFT JOIN plugin_cursors c ON c.plugin_id = p.plugin_id \
              AND c.library_id = p.library_id AND c.hook = ?2 AND c.user_id = u.user_id \
         WHERE {READY} AND {} \
         AND EXISTS (SELECT 1 FROM search_events e WHERE e.library_id = p.library_id \
                     AND e.user_id = u.user_id AND e.seq > coalesce(c.position, 0) \
                     AND ((e.kind <> 'stopped' AND e.at >= ss.shared_at AND {} AND {}) \
                          OR (e.kind = 'stopped' AND e.plugin_id = p.plugin_id)))",
        granted_to_plugin("searched"),
        granted_to_plugin("searchActivity"),
        granted_here("libraryRead"),
    );
    let mut due = Vec::new();
    for (hook, sql) in [
        (Hook::TracksChanged, changes("tracksChanged", "track")),
        (Hook::AlbumsChanged, changes("albumsChanged", "album")),
        (Hook::ArtistsChanged, changes("artistsChanged", "artist")),
        (Hook::ScanFinished, scans),
        (Hook::Schedule, schedules),
        (Hook::Played, played),
        (Hook::Playing, playing),
        (Hook::Searched, searched),
    ] {
        let mut statement = conn.prepare_cached(&sql)?;
        let pairs = statement.query_map(params![now, hook.name()], |row| {
            Ok(Due {
                hook,
                plugin: row.get(0)?,
                library: row.get(1)?,
                position: row.get(2)?,
                failures: row.get(3)?,
                user: row.get(4)?,
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
            let (next, c) = batch(conn, due.library, due.position, "track")?;
            let event = TracksChanged { changed: c.changed, removed: c.removed };
            (next, (!c.empty).then_some(Event::TracksChanged(event)))
        }
        Hook::AlbumsChanged => {
            let (next, c) = batch(conn, due.library, due.position, "album")?;
            let event = AlbumsChanged { changed: c.changed, removed: c.removed };
            (next, (!c.empty).then_some(Event::AlbumsChanged(event)))
        }
        Hook::ArtistsChanged => {
            let (next, c) = batch(conn, due.library, due.position, "artist")?;
            let event = ArtistsChanged { changed: c.changed, removed: c.removed };
            (next, (!c.empty).then_some(Event::ArtistsChanged(event)))
        }
        Hook::ScanFinished => match scans(conn, due.library, due.position, now)? {
            Some((next, totals)) => (next, Some(Event::ScanFinished(totals))),
            None => (due.position, None),
        },
        Hook::Schedule => (now, Some(Event::Scheduled)),
        Hook::Played => {
            let (next, plays) = plays(conn, &due.plugin, due.library, due.user, due.position)?;
            (next, (!plays.is_empty()).then_some(Event::Played(plays)))
        }
        Hook::Searched => searches(conn, &due.plugin, due.library, due.user, due.position)?,
        // One that ended or went stale since it was found due is no longer news.
        Hook::Playing => {
            match playing(conn, &due.plugin, due.library, due.user, due.position, now)? {
                Some((next, started)) => (next, Some(Event::Playing(started))),
                None => (due.position, None),
            }
        }
    })
}

/// Entities of one kind added or changed, and removed.
pub struct Changes {
    pub changed: Vec<u64>,
    pub removed: Vec<u64>,
    pub empty: bool,
}

/// The next batch of changes to `library`'s entities of type `entity` (`track`, `album`, or
/// `artist`) after `position`: the position after it, and the IDs it names.
pub fn batch(
    conn: &Connection,
    library: i64,
    position: i64,
    entity: &str,
) -> rusqlite::Result<(i64, Changes)> {
    let rows: Vec<(i64, i64, String)> = conn
        .prepare_cached(
            "SELECT seq, entity_id, op FROM library_changes \
             WHERE library_id = ?1 AND entity_type = ?2 AND seq > ?3 ORDER BY seq LIMIT ?4",
        )?
        .query_map(params![library, entity, position, BATCH], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let next = rows.last().map_or(position, |(seq, ..)| *seq);
    let mut changes = Changes { changed: Vec::new(), removed: Vec::new(), empty: rows.is_empty() };
    for (_, id, op) in rows {
        match op.as_str() {
            "delete" => changes.removed.push(id as u64),
            _ => changes.changed.push(id as u64),
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
