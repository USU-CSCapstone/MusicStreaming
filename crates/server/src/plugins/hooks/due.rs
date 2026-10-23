//! Which hooks are due, and how each one's event is read.
//!
//! A hook is due for a plugin in a library when the plugin is enabled there, the hook and the
//! permissions it needs ([`Permission::needs`]) are granted, it is not waiting to retry a
//! failure, and there is something past its position: changes of the kind it asked for, a
//! finished scan, its interval gone by, plays ended by a user who connected it, a play one of
//! them has just started and is still playing, or searches by a user who shares them with it.
//! Each user has a position of their own in the personal hooks ([`Hook::Personal`]).

use jewelcase_plugins::{Event, Hook, Permission};
use rusqlite::{Connection, params};

use super::Due;
use super::changes::{changed, scans};
use super::playing::{deliverable, playing};
use super::plays::plays;
use super::searches::searches;
use crate::db::{Text, reaches};

/// Reads what a due hook delivers at the time given: the position once it is handled, and its
/// event. No event means nothing for the plugin, and the position moves on anyway.
pub(super) type Read = fn(&Connection, &Due, i64) -> rusqlite::Result<(i64, Option<Event>)>;

/// The pair's position in the hook `?2`, and the user it is for, named `position` and `user_id`
/// for [`still`]. A personal hook pairs it with each user who connected the plugin, as `s`; a
/// library hook's user is 0.
fn position(hook: Hook) -> String {
    let (users, user) = match hook {
        Hook::Personal => ("JOIN plugin_connections s ON s.plugin_id = p.plugin_id", "s.user_id"),
        Hook::Library => ("", "0"),
    };
    format!(
        "coalesce(c.position, 0) AS position, {user} AS user_id \
         FROM plugin_libraries p {users} \
         LEFT JOIN plugin_cursors c ON c.plugin_id = p.plugin_id \
              AND c.library_id = p.library_id AND c.hook = ?2 AND c.user_id = {user}"
    )
}

/// Each of `permissions` granted to the pair's plugin, in the pair's library or to the plugin
/// itself (library 0), as conditions to add to a `WHERE`. The names are written into the SQL
/// rather than bound: they are serde's names for the enum's variants, a closed set, so no outside
/// text reaches the query.
fn granted(permissions: &[Permission]) -> String {
    permissions
        .iter()
        .map(|permission| {
            format!(
                " AND EXISTS (SELECT 1 FROM plugin_grants g WHERE g.plugin_id = p.plugin_id \
                 AND g.library_id IN (0, p.library_id) AND g.permission = '{}')",
                permission.name()
            )
        })
        .collect()
}

/// `hook` granted, and every permission it needs ([`Permission::needs`]).
fn hook_granted(hook: Permission) -> String {
    granted(&[hook]) + &granted(hook.needs())
}

const READY: &str = "p.enabled AND (c.retry_at IS NULL OR c.retry_at <= ?1)";

/// The user `s` who connected the plugin reaches the pair's library, so it never acts with their
/// settings in a library they cannot reach.
const CONNECTED_REACHES: &str = concat!(
    " AND ",
    reaches!(
        admin: "(SELECT a.role <> 'user' FROM users a WHERE a.id = s.user_id)",
        user: "s.user_id",
        library: "p.library_id"
    )
);

/// The same for the user `u` whose searches are shared or forgotten.
const SHARER_REACHES: &str = concat!(
    " AND ",
    reaches!(
        admin: "(SELECT a.role <> 'user' FROM users a WHERE a.id = u.user_id)",
        user: "u.user_id",
        library: "p.library_id"
    )
);

/// Whether any plugin is enabled in any library, which no hook can be due without.
pub fn any_enabled(conn: &Connection) -> rusqlite::Result<bool> {
    let sql = "SELECT EXISTS (SELECT 1 FROM plugin_libraries WHERE enabled)";
    conn.prepare_cached(sql)?.query_row([], |row| row.get(0))
}

/// Every pair a hook is due for at `now`, with its position and user.
pub fn all(conn: &Connection, now: i64) -> rusqlite::Result<Vec<Due>> {
    let mut due = Vec::new();
    for (hook, sql, read) in hooks() {
        let mut statement = conn.prepare_cached(&sql)?;
        let pairs = statement.query_map(params![now, Text(hook)], |row| {
            Ok(Due {
                hook,
                read,
                plugin: row.get(0)?,
                library: row.get(1)?,
                position: row.get(2)?,
                // A library hook's user is 0.
                user: Some(row.get(3)?).filter(|&user| user != 0),
            })
        })?;
        due.extend(pairs.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    Ok(due)
}

/// Whether `due` is still due at `now`, for the same user and from the same position, found by
/// its hook's query again: the plugin may have been disabled, a grant revoked, its user's access
/// to the library ended, or the plugin installed again since.
pub fn still(conn: &Connection, due: &Due, now: i64) -> rusqlite::Result<bool> {
    let row = hooks().into_iter().find(|(hook, ..)| *hook == due.hook);
    let (hook, sql, _) = row.expect("each hook has a row");
    let found = format!(
        "SELECT EXISTS (SELECT 1 FROM ({sql}) \
         WHERE plugin_id = ?3 AND library_id = ?4 AND position = ?5 AND user_id = ?6)"
    );
    let (plugin, library, position, user) =
        (&due.plugin, due.library, due.position, due.user.unwrap_or(0));
    conn.prepare_cached(&found)?
        .query_row(params![now, Text(hook), plugin, library, position, user], |row| row.get(0))
}

/// Each hook, with its query for the pairs it is due for and how its event is read. The query
/// is given the time as `?1` and the hook's name as `?2`, and finds the plugin, the library, the
/// position, and the user.
pub(super) fn hooks() -> [(Permission, String, Read); 8] {
    use Permission::*;
    let pair = position(Hook::Library);
    // Changes of one kind of entity in the library, for the hook that asked for them.
    let changes = |hook: Permission, entity: &str| {
        format!(
            "SELECT p.plugin_id, p.library_id, {pair} WHERE {READY}{} \
             AND EXISTS (SELECT 1 FROM library_changes f \
                         WHERE f.library_id = p.library_id AND f.entity_type = '{entity}' \
                         AND f.seq > coalesce(c.position, 0))",
            hook_granted(hook),
        )
    };
    // Only scans finished before now, so one finishing this millisecond is never skipped.
    let finished = format!(
        "SELECT p.plugin_id, p.library_id, {pair} WHERE {READY}{} \
         AND EXISTS (SELECT 1 FROM scans s WHERE s.library_id = p.library_id \
                     AND s.state = 'completed' AND s.finished_at > coalesce(c.position, 0) \
                     AND s.finished_at < ?1)",
        hook_granted(ScanFinished),
    );
    // At once if it has never run; after that at the interval from its manifest.
    let scheduled = format!(
        "SELECT p.plugin_id, p.library_id, {pair} WHERE {READY}{} \
         AND (c.position IS NULL OR c.position + 60000 * \
              (SELECT json_extract(r.value, '$.everyMinutes') \
               FROM plugins pl, json_each(pl.manifest, '$.permissions') r \
               WHERE pl.id = p.plugin_id AND json_extract(r.value, '$.permission') = ?2) <= ?1)",
        hook_granted(Schedule),
    );
    // For each user who connected it, plays they ended in the library since they did.
    let ended = format!(
        "SELECT p.plugin_id, p.library_id, {} WHERE {READY}{}{CONNECTED_REACHES} \
         AND EXISTS (SELECT 1 FROM play_events e WHERE e.library_id = p.library_id \
                     AND e.user_id = s.user_id AND e.kind = 'ended' \
                     AND e.seq > coalesce(c.position, 0) AND e.reported_at >= s.connected_at)",
        position(Hook::Personal),
        hook_granted(Played),
    );
    // For each user who connected it, a play they started in the library that is still fresh.
    let started = format!(
        "SELECT p.plugin_id, p.library_id, {} WHERE {READY}{}{CONNECTED_REACHES} \
         AND EXISTS (SELECT 1 FROM play_events st \
                     JOIN plays pl ON pl.user_id = st.user_id AND pl.id = st.play_id WHERE {})",
        position(Hook::Personal),
        hook_granted(Playing),
        deliverable("p.library_id", "s.user_id", "coalesce(c.position, 0)"),
    );
    // For each user who shares their searches with it, what they searched or removed since; and
    // for each who stopped, the request to forget. The hook's grant holds for both, but what it
    // needs holds only for the searches: a request to forget carries nothing of the user's.
    let shared = format!(
        "SELECT p.plugin_id, p.library_id, coalesce(c.position, 0) AS position, u.user_id \
         FROM plugin_libraries p \
         JOIN (SELECT plugin_id, user_id FROM plugin_connections \
               WHERE searches_shared_at IS NOT NULL \
               UNION SELECT plugin_id, user_id FROM search_events WHERE kind = 'stopped') u \
              ON u.plugin_id = p.plugin_id \
         LEFT JOIN plugin_connections s ON s.plugin_id = p.plugin_id AND s.user_id = u.user_id \
         LEFT JOIN plugin_cursors c ON c.plugin_id = p.plugin_id \
              AND c.library_id = p.library_id AND c.hook = ?2 AND c.user_id = u.user_id \
         WHERE {READY}{}{SHARER_REACHES} \
         AND EXISTS (SELECT 1 FROM search_events e WHERE e.library_id = p.library_id \
                     AND e.user_id = u.user_id AND e.seq > coalesce(c.position, 0) \
                     AND ((e.kind <> 'stopped' AND e.created_at >= s.searches_shared_at{}) \
                          OR (e.kind = 'stopped' AND e.plugin_id = p.plugin_id)))",
        granted(&[Searched]),
        granted(Searched.needs()),
    );
    [
        (TracksChanged, changes(TracksChanged, "track"), |conn, due, _| {
            changed(conn, due, "track", Event::TracksChanged)
        }),
        (AlbumsChanged, changes(AlbumsChanged, "album"), |conn, due, _| {
            changed(conn, due, "album", Event::AlbumsChanged)
        }),
        (ArtistsChanged, changes(ArtistsChanged, "artist"), |conn, due, _| {
            changed(conn, due, "artist", Event::ArtistsChanged)
        }),
        (ScanFinished, finished, |conn, due, now| {
            Ok(match scans(conn, due.library, due.position, now)? {
                Some((next, totals)) => (next, Some(Event::ScanFinished(totals))),
                None => (due.position, None),
            })
        }),
        (Schedule, scheduled, |_, _, now| Ok((now, Some(Event::Scheduled)))),
        (Played, ended, |conn, due, _| {
            let Some(user) = due.user else { return Ok((due.position, None)) };
            let (next, plays) = plays(conn, &due.plugin, due.library, user, due.position)?;
            Ok((next, (!plays.is_empty()).then_some(Event::Played(plays))))
        }),
        // One that ended or went stale since it was found due is no longer news.
        (Playing, started, |conn, due, now| {
            let Some(user) = due.user else { return Ok((due.position, None)) };
            Ok(match playing(conn, &due.plugin, due.library, user, due.position, now)? {
                Some((next, started)) => (next, Some(Event::Playing(started))),
                None => (due.position, None),
            })
        }),
        (Searched, shared, |conn, due, _| {
            let Some(user) = due.user else { return Ok((due.position, None)) };
            searches(conn, &due.plugin, due.library, user, due.position)
        }),
    ]
}
