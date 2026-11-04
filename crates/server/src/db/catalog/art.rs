//! Album art, and artist images and biographies, resolved from the tracks
//! (`design/database.md` §3). Tracks offer album art in one order: present before missing, then
//! by disc, a track with none first, then by track number, a track with none last, then by path
//! and id. An album takes the first art, except that among present tracks, and among missing
//! ones, embedded art comes before any `cover.*`. An artist takes the image and biography of
//! the first album it is an album artist of that has a present track offering one. A playlist
//! takes the first four distinct album images among its items, which follow its albums' art.

use std::collections::{BTreeSet, HashSet};

use rusqlite::{Connection, OptionalExtension, Result, Transaction, params};

/// The album's art: the first in track order, read from the head of the index that holds that
/// order for the tracks offering it (`tracks_with_art`). Returns whether it changed.
pub fn album(tx: &Transaction<'_>, album: i64) -> Result<bool> {
    // By the album's key alone: an `UPDATE … FROM` that skips an unchanged image is a join,
    // which planner statistics gathered over a few tracks can turn into a scan of every album.
    let before: Option<i64> = tx
        .prepare_cached("SELECT image_id FROM albums WHERE id = ?1")?
        .query_row([album], |r| r.get(0))
        .optional()?
        .flatten();
    let after: Option<i64> = tx
        .prepare_cached(
            "UPDATE albums SET \
             image_id = (SELECT image_id FROM tracks WHERE album_id = ?1 AND image_id IS NOT NULL \
                 ORDER BY missing_since IS NOT NULL, image_embedded DESC, disc_number, \
                 ifnull(track_number, 9223372036854775807), path, id LIMIT 1) \
             WHERE id = ?1 RETURNING image_id",
        )?
        .query_row([album], |r| r.get(0))
        .optional()?
        .flatten();
    Ok(before != after)
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

/// The ID of the mosaic of `ids`, the first of which is the top left, as a playlist's
/// `image_key` keeps it and the spec's `ImageRef` carries it: `m` and the four image IDs, each as
/// `Id` spells it, joined by `-`.
pub fn mosaic_id(ids: [i64; 4]) -> String {
    let [a, b, c, d] = ids;
    format!("m{a}-{b}-{c}-{d}")
}

/// Sets playlist `id`'s `image_key` from its items as they are now: with four distinct album
/// images among them, the mosaic of the first four in item order; with one to three, the first
/// of them alone; with none, `NULL`. The ID changes exactly when the images it is made of do, so
/// an artwork cached under it never goes stale. Reads items in order and stops at the fourth
/// image, so it reads the playlist's start unless few of its tracks have art. A Liked Songs
/// keeps none: its artwork is a heart clients draw (`requirements/playlists.md` §11).
pub fn playlist(conn: &Connection, id: i64) -> Result<()> {
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
        "UPDATE playlists SET image_key = ?2 WHERE id = ?1 AND kind = 'saved' \
         AND image_key IS NOT ?2",
    )?
    .execute(params![id, key])?;
    Ok(())
}

/// The artwork of every playlist in `lib` that holds one of `moved`, tracks moved onto another
/// album, or a track of one of `albums`, whose art changed. A track's playlists are one probe of
/// `playlist_items_by_track`, so the cost follows the change; an album's are read through its
/// tracks (`tracks_by_album_order`), which costs the album, but only when its art changes.
/// Neither the version nor the modified time moves: the user changed nothing.
pub fn playlists(
    tx: &Transaction<'_>,
    lib: i64,
    moved: &HashSet<i64>,
    albums: &[i64],
) -> Result<()> {
    let mut of_track = tx.prepare_cached(
        "SELECT DISTINCT playlist_id FROM playlist_items WHERE library_id = ?1 AND track_id = ?2",
    )?;
    // Not filtered by `lib`, which would lead the planner to an index on the library, reading
    // every track of it: the album is `lib`'s, and the join keeps to its library.
    let mut of_album = tx.prepare_cached(
        "SELECT DISTINCT i.playlist_id FROM tracks t \
         CROSS JOIN playlist_items i ON i.library_id = t.library_id AND i.track_id = t.id \
         WHERE t.album_id = ?1",
    )?;
    // Ordered, so a batch writes its playlists in the same order whatever the sets'.
    let mut ids = BTreeSet::new();
    for &track in moved {
        for id in of_track.query_map([lib, track], |r| r.get::<_, i64>(0))? {
            ids.insert(id?);
        }
    }
    for &album in albums {
        for id in of_album.query_map([album], |r| r.get::<_, i64>(0))? {
            ids.insert(id?);
        }
    }
    for id in ids {
        playlist(tx, id)?;
    }
    Ok(())
}
