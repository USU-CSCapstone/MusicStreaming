//! Each user's Liked Songs: the playlist in each library whose items are their likes
//! (`requirements/playlists.md` §11). Reading it and making it are shared, so a device marks liked
//! tracks as the server does and makes the playlist the same way.

use jewelcase_core::{sort_key, words};
use rusqlite::{Connection, OptionalExtension, Result, params};

/// The title every Liked Songs has, which cannot be changed.
pub const TITLE: &str = "Liked Songs";

/// The ID and version of `user`'s Liked Songs in `library`, if they have one.
pub fn find(conn: &Connection, user: i64, library: i64) -> Result<Option<(i64, i64)>> {
    conn.prepare_cached(
        "SELECT id, version FROM playlists WHERE user_id = ?1 AND library_id = ?2 AND kind = 'liked'",
    )?
    .query_row([user, library], |row| Ok((row.get(0)?, row.get(1)?)))
    .optional()
}

/// Whether playlist `id` is a Liked Songs.
pub fn is_liked(conn: &Connection, id: i64) -> Result<bool> {
    conn.prepare_cached("SELECT kind = 'liked' FROM playlists WHERE id = ?1")?
        .query_row([id], |row| row.get(0))
}

/// Writes `user`'s Liked Songs `id` in `library`, empty and pinned, created at `now`. They have
/// none there. Whatever tells the account's devices of it is the caller's.
pub fn create(conn: &Connection, user: i64, library: i64, id: i64, now: i64) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO playlists (id, library_id, user_id, kind, title, folded, sort_key, pinned, \
         created_at, modified_at) VALUES (?1, ?2, ?3, 'liked', ?4, ?5, ?6, 1, ?7, ?7)",
    )?
    .execute(params![
        id,
        library,
        user,
        TITLE,
        words(TITLE).join(" "),
        sort_key(TITLE, None).as_bytes(),
        now
    ])?;
    Ok(())
}
