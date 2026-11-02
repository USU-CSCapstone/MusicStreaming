//! A playlist's artwork: the generated mosaic of its first album images (`requirements/playlists.md`
//! §6), whose ID is kept on the playlist row so that reading a playlist costs nothing extra.

use std::collections::HashSet;

use rusqlite::{Connection, params};

use crate::api::images::mosaic_id;

/// Sets playlist `id`'s `image_key` from its items as they are now: with four distinct album
/// images among them, the mosaic of the first four in item order; with one to three, the first
/// of them alone; with none, `NULL`. The ID changes exactly when the images it is made of do, so
/// an artwork cached under it never goes stale. Reads items in order and stops at the fourth
/// image, so it reads the playlist's start unless few of its tracks have art.
pub fn refresh(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    let mut images: Vec<i64> = Vec::with_capacity(4);
    {
        // The order is pinned: items by key, each track and album by their keys.
        let mut select = conn.prepare_cached(
            "SELECT a.image_id FROM playlist_items i \
             CROSS JOIN tracks t ON t.library_id = i.library_id AND t.id = i.track_id \
             CROSS JOIN albums a ON a.library_id = t.library_id AND a.id = t.album_id \
             WHERE i.playlist_id = ?1 AND a.image_id IS NOT NULL ORDER BY i.key",
        )?;
        let mut rows = select.query([id])?;
        while images.len() < 4 {
            let Some(row) = rows.next()? else { break };
            let image = row.get(0)?;
            if !images.contains(&image) {
                images.push(image);
            }
        }
    }
    let key = match <[i64; 4]>::try_from(images.as_slice()) {
        Ok(four) => Some(mosaic_id(four)),
        Err(_) => images.first().map(i64::to_string),
    };
    conn.prepare_cached(
        "UPDATE playlists SET image_key = ?2 WHERE id = ?1 AND image_key IS NOT ?2",
    )?
    .execute(params![id, key])?;
    Ok(())
}

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
