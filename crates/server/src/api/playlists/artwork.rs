//! A playlist's artwork: the generated mosaic of its first album images (`requirements/playlists.md`
//! §6), whose ID is kept on the playlist row so that reading a playlist costs nothing extra. The
//! catalog keeps that ID (`db::catalog::playlist_artwork`), on an edit of the items and on a scan
//! that changes their albums' art; what is here reads it.

use std::collections::HashSet;

use rusqlite::{Connection, params};

/// Whether `user` has a playlist in `library` whose artwork is `key`, the only way a mosaic may
/// be composed for them: it bounds the mosaics that can be made and tells no one what another
/// user holds.
pub fn held(conn: &Connection, user: i64, library: i64, key: &str) -> rusqlite::Result<bool> {
    conn.prepare_cached(
        "SELECT 1 FROM playlists WHERE user_id = ?1 AND library_id = ?2 AND image_key = ?3",
    )?
    .exists(params![user, library, key])
}

/// The mosaic IDs that are the artwork of `user`'s playlists in `library`.
pub fn mosaics_of(conn: &Connection, user: i64, library: i64) -> rusqlite::Result<HashSet<String>> {
    conn.prepare_cached(
        "SELECT image_key FROM playlists \
         WHERE user_id = ?1 AND library_id = ?2 AND image_key GLOB 'm*'",
    )?
    .query_map([user, library], |row| row.get(0))?
    .collect()
}

/// Every playlist's mosaic ID, as `(library, user, id)`.
pub fn all_mosaics(conn: &Connection) -> rusqlite::Result<Vec<(i64, i64, String)>> {
    conn.prepare_cached(
        "SELECT library_id, user_id, image_key FROM playlists WHERE image_key GLOB 'm*'",
    )?
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
    .collect()
}
