//! Album art and artist images and biographies, resolved from the tracks (`design/database.md`
//! §3). Tracks offer theirs in one order: present before missing, then by disc, a track with
//! none first, then by track number, a track with none last, then by path and id. An album takes
//! the first art in that order, except that among present tracks, and among missing ones,
//! embedded art comes before any `cover.*`. It points at its first track with an artist image and
//! its first with a biography, so an artist chooses among its albums' offers rather than reading
//! every track that credits it.

use rusqlite::{Result, Transaction, params};

/// The album's art and the tracks that offer its artists an image and a biography, from its
/// tracks.
pub fn album(tx: &Transaction<'_>, album: i64) -> Result<()> {
    tx.execute(
        "UPDATE albums SET \
         image_id = (SELECT image_id FROM tracks WHERE album_id = ?1 AND image_id IS NOT NULL \
             ORDER BY missing_since IS NOT NULL, image_embedded DESC, disc_number, \
             ifnull(track_number, 9223372036854775807), path, id LIMIT 1), \
         artist_image_track_id = (SELECT id FROM tracks WHERE album_id = ?1 AND artist_image_id IS NOT NULL \
             ORDER BY missing_since IS NOT NULL, disc_number, \
             ifnull(track_number, 9223372036854775807), path, id LIMIT 1), \
         biography_track_id = (SELECT id FROM tracks WHERE album_id = ?1 AND biography_id IS NOT NULL \
             ORDER BY missing_since IS NOT NULL, disc_number, \
             ifnull(track_number, 9223372036854775807), path, id LIMIT 1) \
         WHERE id = ?1",
        [album],
    )?;
    Ok(())
}

/// The artist's image and biography: those of the first track its albums offer, in the tracks'
/// order. The unknown artist, not `named`, has neither.
pub fn artist(tx: &Transaction<'_>, artist: i64, named: bool) -> Result<()> {
    tx.execute(
        "UPDATE artists SET \
         image_id = (SELECT t.artist_image_id FROM album_artists aa JOIN albums al ON al.id = aa.album_id \
             JOIN tracks t ON t.id = al.artist_image_track_id WHERE aa.artist_id = ?1 AND ?2 \
             ORDER BY t.missing_since IS NOT NULL, t.disc_number, \
             ifnull(t.track_number, 9223372036854775807), t.path, t.id LIMIT 1), \
         biography = (SELECT b.text FROM album_artists aa JOIN albums al ON al.id = aa.album_id \
             JOIN tracks t ON t.id = al.biography_track_id JOIN biographies b ON b.id = t.biography_id \
             WHERE aa.artist_id = ?1 AND ?2 \
             ORDER BY t.missing_since IS NOT NULL, t.disc_number, \
             ifnull(t.track_number, 9223372036854775807), t.path, t.id LIMIT 1) \
         WHERE id = ?1",
        params![artist, named],
    )?;
    Ok(())
}
