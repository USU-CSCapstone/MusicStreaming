//! What a `playing` hook delivers: the newest play a user who connected the plugin has started
//! in a library, from the feed of play starts (`0014_plugin_playing.sql`).
//!
//! "Now playing" that arrives late is wrong rather than late, so unlike every other hook this
//! one never catches up. A start is delivered only while it is under [`FRESH_MS`] old and still
//! playing, and the newest passes over every start before it (`design/hooks.md` §1.1).

use jewelcase_plugins::Playing;
use rusqlite::{Connection, OptionalExtension, params};

use super::super::library::{TRACK_COLUMNS, track};

/// How long after the server first hears of a play it is still worth announcing.
pub const FRESH_MS: i64 = 60_000;

/// The condition, in SQL, for a start `st` of play `pl` to be delivered: by `user` in `library`
/// after `position`, each an SQL expression; since they connected, as `s.connected_at` says;
/// under [`FRESH_MS`] old at the time `?1`; and still playing.
pub fn deliverable(library: &str, user: &str, position: &str) -> String {
    format!(
        "st.library_id = {library} AND st.user_id = {user} AND st.seq > {position} \
         AND pl.ended IS NULL AND st.at >= max(s.connected_at, ?1 - {FRESH_MS})"
    )
}

/// The newest start `user` may be told about in `library` after `position`, and the position
/// that passes it and every start before it; none if there is no such start.
pub fn playing(
    conn: &Connection,
    plugin: &str,
    library: i64,
    user: i64,
    position: i64,
    now: i64,
) -> rusqlite::Result<Option<(i64, Playing)>> {
    let deliverable = deliverable("?2", "?3", "?4");
    conn.prepare_cached(&format!(
        "{TRACK_COLUMNS}, pl.started_at, st.seq FROM play_starts st \
         JOIN plays pl ON pl.id = st.play_id JOIN tracks t ON t.id = pl.track_id \
         JOIN albums al ON al.id = t.album_id \
         JOIN plugin_user_settings s ON s.plugin_id = ?5 AND s.user_id = st.user_id \
         WHERE {deliverable} ORDER BY st.seq DESC LIMIT 1"
    ))?
    .query_row(params![now, library, user, position, plugin], |row| {
        let started_at = row.get::<_, i64>(13)? as u64;
        Ok((row.get(14)?, Playing { track: track(row)?, started_at }))
    })
    .optional()
}
