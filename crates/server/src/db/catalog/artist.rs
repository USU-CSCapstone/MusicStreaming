//! An artist's name, sort key, image and biography, from the credits that name it
//! (`design/database.md` §3).

use jewelcase_core::sort_key;
use rusqlite::{OptionalExtension, Result, Transaction, params};

use super::art;
use crate::db::feed::{self, Entity, Op};

/// The artist's displayed name and sort key from the spellings and sort keys its credits count,
/// and its image and biography from its albums ([`art::artist`]), or its deletion when no track
/// credits it. Whether one does is read from the credits themselves, so a wrong count can never
/// delete a credited artist. The unknown artist has no image or biography. A changed sort key
/// reaches the albums the artist heads and the tracks crediting it first
/// ([`follow_artist_sort_key`]).
pub(super) fn refresh_artist(tx: &Transaction<'_>, lib: i64, artist: i64, now: i64) -> Result<()> {
    let Some((credited, name, tagged_sort, old_sort)) = tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM track_artists WHERE artist_id = ?1) \
                 OR EXISTS (SELECT 1 FROM track_album_artists WHERE artist_id = ?1), \
             (SELECT name FROM artist_spellings WHERE artist_id = ?1 ORDER BY uses DESC, name LIMIT 1), \
             (SELECT sort_key FROM artist_sort_keys WHERE artist_id = ?1 ORDER BY uses DESC, sort_key LIMIT 1), \
             sort_key FROM artists WHERE id = ?1",
        )?
        .query_row([artist], |r| {
            Ok((
                r.get::<_, bool>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?,
                r.get::<_, Vec<u8>>(3)?,
            ))
        })
        .optional()?
    else {
        return Ok(());
    };
    if !credited {
        tx.prepare_cached("DELETE FROM artists WHERE id = ?1")?.execute([artist])?;
        return feed::record(tx, lib, Entity::Artist, artist, Op::Delete, now);
    }
    // The tagged sort value where this artist is the primary credit, else derived from the name.
    let sort_key = tagged_sort
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| sort_key(name.as_deref().unwrap_or(""), None).into_bytes());
    tx.prepare_cached("UPDATE artists SET name = ?2, sort_key = ?3 WHERE id = ?1")?
        .execute(params![artist, name, sort_key])?;
    if sort_key != old_sort {
        follow_artist_sort_key(tx, lib, artist, &sort_key, now)?;
    }
    art::artist(tx, artist, name.is_some())?;
    feed::record(tx, lib, Entity::Artist, artist, Op::Upsert, now)
}

/// Gives every album `artist` heads, as its first album artist, and every track crediting it
/// first the artist's `sort_key`, and records each of those albums in the feed. Albums the
/// batch touched take it again as they are recomputed; this reaches the rest. It costs the
/// artist's albums and tracks, so the caller runs it only when the artist's key changes. Each row
/// is written by id, so no planner statistics can turn it into a scan of every album or track
/// (`design/database.md` §4).
fn follow_artist_sort_key(
    tx: &Transaction<'_>,
    lib: i64,
    artist: i64,
    sort_key: &[u8],
    now: i64,
) -> Result<()> {
    let mut albums = tx.prepare_cached(
        "SELECT album_id FROM album_artists WHERE artist_id = ?1 AND position = 0",
    )?;
    let mut album_key =
        tx.prepare_cached("UPDATE albums SET artist_sort_key = ?2 WHERE id = ?1")?;
    for album in albums.query_map([artist], |r| r.get::<_, i64>(0))? {
        let album = album?;
        album_key.execute(params![album, sort_key])?;
        feed::record(tx, lib, Entity::Album, album, Op::Upsert, now)?;
    }
    let mut tracks = tx.prepare_cached(
        "SELECT track_id FROM track_artists WHERE artist_id = ?1 AND position = 0",
    )?;
    let mut track_key =
        tx.prepare_cached("UPDATE tracks SET artist_sort_key = ?2 WHERE id = ?1")?;
    for track in tracks.query_map([artist], |r| r.get::<_, i64>(0))? {
        track_key.execute(params![track?, sort_key])?;
    }
    Ok(())
}
