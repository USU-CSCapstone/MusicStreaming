//! Album art and labels, and artist images and biographies, resolved from the tracks
//! (`design/database.md` §3). Tracks offer theirs in one order: present before missing, then by
//! disc, a track with none first, then by track number, a track with none last, then by path and
//! id. An album takes the labels of the first track in that order that has any, and the first
//! art, except that among present tracks, and among missing ones, embedded art comes before any
//! `cover.*`. It points at its first track with an artist image and its first with a biography,
//! so an artist chooses among its albums' offers rather than reading every track that credits it.

use rusqlite::{Result, Transaction, params};

/// The album's art, its labels (none when no track has any), and the tracks that offer its
/// artists an image and a biography: each the first in track order, read from the head of the
/// index that holds that order for the tracks offering it (`tracks_with_art` and the others).
pub fn album(tx: &Transaction<'_>, album: i64) -> Result<()> {
    tx.prepare_cached(
        "UPDATE albums SET \
         image_id = (SELECT image_id FROM tracks WHERE album_id = ?1 AND image_id IS NOT NULL \
             ORDER BY missing_since IS NOT NULL, image_embedded DESC, disc_number, \
             ifnull(track_number, 9223372036854775807), path, id LIMIT 1), \
         artist_image_track_id = (SELECT id FROM tracks WHERE album_id = ?1 AND artist_image_id IS NOT NULL \
             ORDER BY missing_since IS NOT NULL, disc_number, \
             ifnull(track_number, 9223372036854775807), path, id LIMIT 1), \
         biography_track_id = (SELECT id FROM tracks WHERE album_id = ?1 AND biography_id IS NOT NULL \
             ORDER BY missing_since IS NOT NULL, disc_number, \
             ifnull(track_number, 9223372036854775807), path, id LIMIT 1), \
         labels = ifnull((SELECT labels FROM tracks WHERE album_id = ?1 AND labels <> '[]' \
             ORDER BY missing_since IS NOT NULL, disc_number, \
             ifnull(track_number, 9223372036854775807), path, id LIMIT 1), '[]') \
         WHERE id = ?1",
    )?
    .execute([album])?;
    Ok(())
}

/// The artist's image and biography: those of the first track its albums offer, in the tracks'
/// order. The unknown artist, not `named`, has neither. Each reads the artist's albums and the
/// track each offers by id, so it costs the artist's albums rather than the library's.
pub fn artist(tx: &Transaction<'_>, artist: i64, named: bool) -> Result<()> {
    // Each album and biography is read through a subquery by id, which SQLite answers by id
    // alone. Each track is joined NOT INDEXED, leaving the planner only its id: statistics
    // gathered while no album offered an image or a biography would have it scan the partial
    // indexes of the albums and tracks that do. CROSS JOIN keeps the artist's albums as the outer
    // loop. Statistics that describe `tracks` as a single row would still have the planner scan
    // for the track; the writer never gathers those (`STATISTICS_FROM_TRACKS` in `db.rs`).
    tx.prepare_cached(
        "UPDATE artists SET \
         image_id = (SELECT t.artist_image_id FROM album_artists aa CROSS JOIN tracks t NOT INDEXED \
             ON t.id = (SELECT artist_image_track_id FROM albums WHERE id = aa.album_id) \
             WHERE aa.artist_id = ?1 AND ?2 \
             ORDER BY t.missing_since IS NOT NULL, t.disc_number, \
             ifnull(t.track_number, 9223372036854775807), t.path, t.id LIMIT 1), \
         biography = (SELECT (SELECT text FROM biographies WHERE id = t.biography_id) \
             FROM album_artists aa CROSS JOIN tracks t NOT INDEXED \
             ON t.id = (SELECT biography_track_id FROM albums WHERE id = aa.album_id) \
             WHERE aa.artist_id = ?1 AND ?2 \
             ORDER BY t.missing_since IS NOT NULL, t.disc_number, \
             ifnull(t.track_number, 9223372036854775807), t.path, t.id LIMIT 1) \
         WHERE id = ?1",
    )?
    .execute(params![artist, named])?;
    Ok(())
}
