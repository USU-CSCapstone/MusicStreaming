//! Album art, and artist images and biographies, resolved from the tracks
//! (`design/database.md` §3). Tracks offer album art in one order: present before missing, then
//! by disc, a track with none first, then by track number, a track with none last, then by path
//! and id. An album takes the first art, except that among present tracks, and among missing
//! ones, embedded art comes before any `cover.*`. An artist takes the image and biography of
//! the first album it is an album artist of that has a present track offering one.

use rusqlite::{Result, Transaction, params};

/// The album's art: the first in track order, read from the head of the index that holds that
/// order for the tracks offering it (`tracks_with_art`).
pub fn album(tx: &Transaction<'_>, album: i64) -> Result<()> {
    tx.prepare_cached(
        "UPDATE albums SET \
         image_id = (SELECT image_id FROM tracks WHERE album_id = ?1 AND image_id IS NOT NULL \
             ORDER BY missing_since IS NOT NULL, image_embedded DESC, disc_number, \
             ifnull(track_number, 9223372036854775807), path, id LIMIT 1) \
         WHERE id = ?1",
    )?
    .execute([album])?;
    Ok(())
}

/// The artist's image and biography: those of a present track on one of its albums, none when
/// no album has one. The unknown artist, not `named`, has neither. Each stops at the first
/// match, so it costs the artist's albums at most, not the library's.
pub fn artist(tx: &Transaction<'_>, artist: i64, named: bool) -> Result<()> {
    // CROSS JOIN keeps the artist's albums as the outer loop, and INDEXED BY pins each album's
    // lookup to a partial index holding only the tracks that offer, so each album is one probe
    // and the plan is the same whatever the planner's statistics. No bad plan is known without
    // it; it is insurance. The WHERE repeats each index's predicate, which INDEXED BY requires.
    tx.prepare_cached(
        "UPDATE artists SET \
         image_id = (SELECT t.artist_image_id FROM album_artists aa CROSS JOIN tracks t \
             INDEXED BY tracks_with_artist_image ON t.album_id = aa.album_id \
             WHERE aa.artist_id = ?1 AND ?2 AND t.artist_image_id IS NOT NULL \
             AND t.missing_since IS NULL LIMIT 1), \
         biography = (SELECT (SELECT text FROM biographies WHERE id = t.biography_id) \
             FROM album_artists aa CROSS JOIN tracks t \
             INDEXED BY tracks_with_biography ON t.album_id = aa.album_id \
             WHERE aa.artist_id = ?1 AND ?2 AND t.biography_id IS NOT NULL \
             AND t.missing_since IS NULL LIMIT 1) \
         WHERE id = ?1",
    )?
    .execute(params![artist, named])?;
    Ok(())
}
