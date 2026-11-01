//! A playlist's artwork: the generated mosaic of its first album images (`requirements/playlists.md`
//! §6), whose ID is kept on the playlist row so that reading a playlist costs nothing extra.

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
