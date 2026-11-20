//! Emptying an account being deleted (`design/database.md` §6, §9).
//!
//! A deletion's first write marks the account `deleting`, which signs it out everywhere and
//! frees its name. Its rows then go here, table by table, a chunk per write, so the writer is
//! never held for more than a chunk: one cascading delete of an account with 300,000 plays held
//! it for seconds. Its `users` row goes last, and its cascade takes only the few rows of the
//! tables left to it. The hourly sweep runs this for every account still `deleting`, so a run cut
//! short, by a failed chunk or a stopped server, is finished later.

use rusqlite::{OptionalExtension, Result, Transaction, params};
use tracing::warn;

use crate::db::Database;

#[cfg(test)]
mod tests;

/// Whether account `?1` is being deleted. Every write of the task reads it first, so a bug or a
/// race can never empty a live account.
const DELETING: &str = "SELECT 1 FROM users WHERE id = ?1 AND status = 'deleting'";

/// The task's last statement, which deletes account `?1`'s row and, by cascade, what is left.
const LAST: &str = "DELETE FROM users WHERE id = ?1 AND status = 'deleting'";

/// Each table the task empties, in order, with the statement that deletes up to `?2` of account
/// `?1`'s rows through an index (for `login_attempts`, clears their link to it), and how many
/// rows one write takes.
///
/// Plays go before devices, since deleting a device clears `device_id` on each of its plays,
/// and items before their playlists, since deleting a playlist takes all of its items in one
/// statement. A queue list is up to 4 MB, so they go one a write. `plugin_cursors` has no
/// foreign key to `users`, so nothing else would delete its rows; its key leads with the plugin
/// and library, so its rows are reached through every plugin and library, and the hooks that keep
/// a position per user.
const TABLES: [(&str, &str, usize); 15] = [
    (
        "plays",
        "DELETE FROM plays WHERE rowid IN \
         (SELECT rowid FROM plays WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "playlist_items",
        "DELETE FROM playlist_items WHERE (playlist_id, key) IN \
         (SELECT i.playlist_id, i.key FROM playlists p \
          JOIN playlist_items i ON i.playlist_id = p.id WHERE p.user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "playlists",
        "DELETE FROM playlists WHERE rowid IN \
         (SELECT rowid FROM playlists WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "account_changes",
        "DELETE FROM account_changes WHERE rowid IN \
         (SELECT rowid FROM account_changes WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "hidden_tracks",
        "DELETE FROM hidden_tracks WHERE (user_id, library_id, track_id) IN \
         (SELECT user_id, library_id, track_id FROM hidden_tracks WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "mark_events",
        "DELETE FROM mark_events WHERE rowid IN \
         (SELECT rowid FROM mark_events WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "search_events",
        "DELETE FROM search_events WHERE rowid IN \
         (SELECT rowid FROM search_events WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "recent_searches",
        "DELETE FROM recent_searches WHERE rowid IN \
         (SELECT rowid FROM recent_searches WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "queues",
        "DELETE FROM queues WHERE user_id = ?1 AND queue_id IN \
         (SELECT queue_id FROM queues WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "queue_lists",
        "DELETE FROM queue_lists WHERE rowid IN \
         (SELECT rowid FROM queue_lists WHERE user_id = ?1 LIMIT ?2)",
        1,
    ),
    (
        "idempotency_keys",
        "DELETE FROM idempotency_keys WHERE user_id = ?1 AND key IN \
         (SELECT key FROM idempotency_keys WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "account_feed_starts",
        "DELETE FROM account_feed_starts WHERE user_id = ?1 AND started IN \
         (SELECT started FROM account_feed_starts WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "devices",
        "DELETE FROM devices WHERE rowid IN \
         (SELECT rowid FROM devices WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "login_attempts",
        "UPDATE login_attempts SET user_id = NULL WHERE rowid IN \
         (SELECT rowid FROM login_attempts WHERE user_id = ?1 LIMIT ?2)",
        100,
    ),
    (
        "plugin_cursors",
        "DELETE FROM plugin_cursors WHERE (plugin_id, library_id, hook, user_id) IN \
         (SELECT plugin_id, library_id, hook, user_id FROM plugin_cursors \
          WHERE plugin_id IN (SELECT id FROM plugins) AND library_id IN (SELECT id FROM libraries) \
            AND hook IN ('played', 'playing', 'searched', 'marked') AND user_id = ?1 LIMIT ?2)",
        100,
    ),
];

/// Deletes every row of account `user`, which must be `deleting`, a chunk of one table per
/// write, in the order of [`TABLES`], then its `users` row. Does nothing to an account that is
/// not being deleted. A chunk that fails is logged and ends the run; the next sweep starts it
/// again.
pub async fn empty(db: &Database, user: i64) {
    let mut at = 0;
    loop {
        let chunk = db
            .write(move |tx| {
                let mut next = at;
                step(tx, user, &mut next).map(|done| (done, next))
            })
            .await;
        match chunk {
            Ok((true, _)) => return,
            Ok((false, next)) => at = next,
            Err(error) => {
                warn!(%error, user, "cannot delete part of an account; the next sweep goes on");
                return;
            }
        }
    }
}

/// Deletes the next chunk of account `user`'s rows in table `TABLES[*at]`, or moves `at` on to
/// the next table if it has none left, or, past the last, deletes the account's row. Returns
/// whether the task is over: the row is gone, or the account is not being deleted, in which case
/// nothing changes.
fn step(tx: &Transaction<'_>, user: i64, at: &mut usize) -> Result<bool> {
    if tx.prepare_cached(DELETING)?.query_row([user], |_| Ok(())).optional()?.is_none() {
        return Ok(true);
    }
    let Some(&(_, sql, chunk)) = TABLES.get(*at) else {
        tx.prepare_cached(LAST)?.execute([user])?;
        return Ok(true);
    };
    if tx.prepare_cached(sql)?.execute(params![user, chunk])? == 0 {
        *at += 1;
    }
    Ok(false)
}
