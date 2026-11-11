//! The library and account change feeds (`design/database.md` §5): one row per entity,
//! coalesced on write, in the same transaction as the change. Each feed numbers its rows with
//! positions of its own, which clients hold with the epoch of the server's run that gave them.

use rusqlite::{Connection, OptionalExtension, Result, params};
use serde::Serialize;

use super::{Text, new_id, plugin_view};

/// What changed, stored by its name in `library_changes.entity_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Entity {
    Library,
    Track,
    Album,
    Artist,
    Tag,
}

/// How it changed, stored by its name in `library_changes.op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Upsert,
    Delete,
}

/// Records that `entity` changed, at the library's next position. Its earlier row, if any, is
/// replaced, so the feed carries only the latest position for each entity. An upsert that leaves
/// what plugins see of the entity as it was ([`plugin_view::fingerprint`]) keeps the position
/// plugins last saw it change at, so it moves the entity for devices without waking the change
/// hooks.
pub fn record(
    conn: &Connection,
    library_id: i64,
    entity: Entity,
    id: i64,
    op: Op,
    now: i64,
) -> Result<()> {
    let view = match op {
        Op::Upsert => plugin_view::fingerprint(conn, library_id, entity, id)?,
        Op::Delete => None,
    };
    let position = next_position(conn, Feed::Library(library_id))?;
    // A NULL view never equals one, so a deletion, a kind no hook reads, and a row with no
    // stored view all count as seen by plugins. The old row is read before REPLACE drops it.
    conn.prepare_cached(
        "INSERT OR REPLACE INTO library_changes (library_id, entity_type, entity_id, op, \
         changed_at, plugin_view, plugin_since, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, \
         (SELECT plugin_seq FROM library_changes \
          WHERE library_id = ?1 AND entity_type = ?2 AND entity_id = ?3 AND plugin_view = ?6), ?7)",
    )?
    .execute(params![library_id, Text(entity), id, Text(op), now, view, position])?;
    Ok(())
}

/// What changed in a user's own data, stored by its name in `account_changes.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Playlist,
    PlaylistItem,
    HiddenTrack,
}

/// Records that `user`'s `entity` changed in `library_id`: their playlist for `Kind::Playlist`
/// and `Kind::PlaylistItem`, whose item's key `item_key` names (`0` for the other kinds), or a
/// track for `Kind::HiddenTrack`. It takes the account's next position, and its earlier row, if
/// any, is replaced, so the account feed carries only the latest position for each. Call it in
/// the transaction of the change itself.
pub fn record_account(
    conn: &Connection,
    user: i64,
    library_id: i64,
    kind: Kind,
    entity: i64,
    item_key: i64,
    op: Op,
) -> Result<()> {
    let position = next_position(conn, Feed::Account(user))?;
    conn.prepare_cached(
        "INSERT OR REPLACE INTO account_changes (user_id, library_id, kind, entity_id, \
         item_key, op, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?
    .execute(params![user, library_id, Text(kind), entity, item_key, Text(op), position])?;
    Ok(())
}

/// Records that `user`'s playlist `playlist` was deleted: its item rows leave the feed and the
/// playlist's own row becomes the one tombstone.
pub fn record_playlist_deleted(
    conn: &Connection,
    user: i64,
    library_id: i64,
    playlist: i64,
) -> Result<()> {
    conn.prepare_cached(
        "DELETE FROM account_changes WHERE user_id = ?1 AND entity_id = ?2 \
         AND kind IN ('playlist', 'playlistItem')",
    )?
    .execute(params![user, playlist])?;
    record_account(conn, user, library_id, Kind::Playlist, playlist, 0, Op::Delete)
}

/// One feed a client follows: a library's, or a user's own.
#[derive(Debug, Clone, Copy)]
pub enum Feed {
    Library(i64),
    Account(i64),
}

impl Feed {
    /// The table of the feed's owner, which keeps its head in `feed_head`, the owner's ID, and
    /// the table and owner column that keep its head at each start.
    fn tables(self) -> (&'static str, i64, &'static str, &'static str) {
        match self {
            Feed::Library(id) => ("libraries", id, "library_feed_starts", "library_id"),
            Feed::Account(id) => ("users", id, "account_feed_starts", "user_id"),
        }
    }
}

/// Moves `feed`'s head on by one and returns it: the position of the change being recorded.
fn next_position(conn: &Connection, feed: Feed) -> Result<i64> {
    let (owner, id, ..) = feed.tables();
    conn.prepare_cached(&format!(
        "UPDATE {owner} SET feed_head = feed_head + 1 WHERE id = ?1 RETURNING feed_head"
    ))?
    .query_row([id], |row| row.get(0))
}

/// The position of `feed`'s latest change, 0 before the first. It never goes back.
pub fn head(conn: &Connection, feed: Feed) -> Result<i64> {
    let (owner, id, ..) = feed.tables();
    conn.prepare_cached(&format!("SELECT feed_head FROM {owner} WHERE id = ?1"))?
        .query_row([id], |row| row.get(0))
}

/// Starts a new run of the server: draws its epoch, keeps where every feed is, which is where the
/// run before ended, and returns the epoch. Call it once each time the database is opened.
pub fn start_epoch(conn: &Connection) -> Result<i64> {
    let epoch = new_id()?;
    conn.execute("INSERT INTO feed_epochs (epoch) VALUES (?1)", [epoch])?;
    let started = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO library_feed_starts (started, library_id, head) \
         SELECT ?1, id, feed_head FROM libraries",
        [started],
    )?;
    conn.execute(
        "INSERT INTO account_feed_starts (started, user_id, head) \
         SELECT ?1, id, feed_head FROM users",
        [started],
    )?;
    Ok(epoch)
}

/// Whether a client at position `after` of `feed`, given in run `epoch`, can go on from there:
/// `after` is 0, or the epoch is one of this database's runs and `after` is no further than that
/// run's feed reached. A run this database never saw, as a restored backup does not, or a
/// position past where the run ended here, as one the backup was taken before, cannot.
pub fn continues(conn: &Connection, feed: Feed, epoch: Option<i64>, after: i64) -> Result<bool> {
    if after == 0 {
        return Ok(true);
    }
    let Some(epoch) = epoch else { return Ok(false) };
    let (owner, id, starts, column) = feed.tables();
    // The run ended where the next start found the feed; the current run, at the feed's head.
    let reached: Option<i64> = conn
        .prepare_cached(&format!(
            "SELECT coalesce( \
                 (SELECT coalesce(s.head, 0) FROM feed_epochs AS n \
                  LEFT JOIN {starts} AS s ON s.started = n.started AND s.{column} = ?2 \
                  WHERE n.started > e.started ORDER BY n.started LIMIT 1), \
                 (SELECT feed_head FROM {owner} WHERE id = ?2)) \
             FROM feed_epochs AS e WHERE e.epoch = ?1"
        ))?
        .query_row([epoch, id], |row| row.get(0))
        .optional()?
        .flatten();
    Ok(reached.is_some_and(|reached| after <= reached))
}
