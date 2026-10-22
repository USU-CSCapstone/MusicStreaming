//! Writing one scanned file: its track row, credits, genres, lyrics, and the art and sidecars it
//! resolved, in the batch's transaction.

use jewelcase_core::artists_key;
use jewelcase_scanner::{Lyrics, TrackRecord};
use rusqlite::{Result, Transaction, params};

use super::entities::{find_or_create_album, find_or_create_artist, find_or_create_tag};
use super::roots::Roots;
use crate::db::Json;
use crate::db::catalog::Touched;
use crate::db::feed::{self, Entity, Op};
use row::write_row;

mod row;

/// A track's artist and album-artist credits, each an artist's id and its name as tagged (`None`
/// for the unknown artist), and the album they place the track on.
struct Credits<'a> {
    artists: Vec<(i64, Option<&'a str>)>,
    album_artists: Vec<(i64, Option<&'a str>)>,
    album_id: i64,
}

/// Writes `rec` as a track of `lib`, with its credits, genres, lyrics, and the art, artist image
/// and biography it resolved, records it in the feed at `now`, and adds the albums, artists and
/// tags it pointed at before and after to `touched`, which take their art and biographies from
/// their tracks as they are recomputed. A track whose file's size or mtime changed loses its
/// loudness, peak and waveform and goes back to analyzer version 0, which queues it for analysis
/// again: neither says whether the audio or only the tags changed. A record under a root `lib`
/// doesn't have, or one naming another library's track, is logged and skipped.
pub fn upsert_track(
    roots: &Roots,
    tx: &Transaction<'_>,
    lib: i64,
    rec: &TrackRecord,
    now: i64,
    touched: &mut Touched,
) -> Result<()> {
    let Some(root_id) = roots.root_by_path(lib, &rec.root) else {
        tracing::warn!(path = %rec.path.display(), "track under an unknown root; skipped");
        return Ok(());
    };
    if let Some(id) = rec.id {
        // Whatever it pointed at before may now be orphaned.
        touched.track(tx, lib, id)?;
    }
    let credits = credits(tx, lib, rec, now)?;
    // Touched even if the track is skipped below, so an album or artist made only for it is
    // removed again.
    touched.albums.insert(credits.album_id);
    touched.artists.extend(credits.artists.iter().chain(&credits.album_artists).map(|&(id, _)| id));

    let Some(id) = write_row(tx, lib, root_id, rec, credits.album_id, now)? else {
        tracing::error!(path = %rec.path.display(), "track of another library; skipped");
        return Ok(());
    };
    replace_links(tx, lib, id, &credits, &rec.tags.genres, touched)?;
    replace_lyrics(tx, lib, id, rec.lyrics())?;
    feed::record(tx, lib, Entity::Track, id, Op::Upsert, now)
}

/// Finds or makes the artists `rec` credits, then its album artists as the scanner resolves them,
/// then the album they place it on.
fn credits<'a>(
    tx: &Transaction<'_>,
    lib: i64,
    rec: &'a TrackRecord,
    now: i64,
) -> Result<Credits<'a>> {
    let artists = artist_ids(tx, lib, &rec.tags.artists, now)?;
    let album_artists = artist_ids(tx, lib, rec.tags.album_artists_or_primary(), now)?;
    let artists_key = artists_key(album_artists.iter().map(|(_, name)| name.unwrap_or("")));
    let album_id = find_or_create_album(
        tx,
        lib,
        rec.tags.album.as_deref(),
        &artists_key,
        &rec.sort.album,
        &rec.sort.album_artist,
        now,
    )?;
    Ok(Credits { artists, album_artists, album_id })
}

/// Finds or makes the artist of each of `names`, paired with the name, or the unknown artist
/// (`None`, key "") alone when there are none.
fn artist_ids<'a>(
    tx: &Transaction<'_>,
    lib: i64,
    names: &'a [String],
    now: i64,
) -> Result<Vec<(i64, Option<&'a str>)>> {
    if names.is_empty() {
        return Ok(vec![(find_or_create_artist(tx, lib, None, now)?, None)]);
    }
    names
        .iter()
        .map(|name| {
            let name = Some(name.as_str());
            Ok((find_or_create_artist(tx, lib, name, now)?, name))
        })
        .collect()
}

/// Replaces track `id`'s artist, album-artist and genre links wholesale with `credits` and
/// `genres`, and adds each genre's tag to `touched`.
fn replace_links(
    tx: &Transaction<'_>,
    lib: i64,
    id: i64,
    credits: &Credits<'_>,
    genres: &[String],
    touched: &mut Touched,
) -> Result<()> {
    tx.execute("DELETE FROM track_artists WHERE track_id = ?1", [id])?;
    tx.execute("DELETE FROM track_album_artists WHERE track_id = ?1", [id])?;
    tx.execute("DELETE FROM track_tags WHERE track_id = ?1", [id])?;
    for (pos, (artist_id, name)) in credits.artists.iter().enumerate() {
        tx.execute(
            "INSERT INTO track_artists (library_id, track_id, position, artist_id, artist_name) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT DO NOTHING",
            params![lib, id, pos as i64, artist_id, name.map(str::trim)],
        )?;
    }
    for (pos, (artist_id, name)) in credits.album_artists.iter().enumerate() {
        tx.execute(
            "INSERT INTO track_album_artists (library_id, track_id, position, artist_id, artist_name) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT DO NOTHING",
            params![lib, id, pos as i64, artist_id, name.map(str::trim)],
        )?;
    }
    for genre in genres {
        let tag_id = find_or_create_tag(tx, lib, genre)?;
        tx.execute(
            "INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT DO NOTHING",
            params![lib, id, tag_id, genre.trim()],
        )?;
        touched.tags.insert(tag_id);
    }
    Ok(())
}

/// Replaces track `id`'s lyrics with `lyrics`, or removes them when it is `None`.
fn replace_lyrics(tx: &Transaction<'_>, lib: i64, id: i64, lyrics: Option<&Lyrics>) -> Result<()> {
    tx.execute("DELETE FROM track_lyrics WHERE track_id = ?1", [id])?;
    if let Some(lyrics) = lyrics {
        let (plain, synced) = match lyrics {
            Lyrics::Plain(text) => (Some(text), None),
            Lyrics::Synced(lines) => (None, Some(Json(lines))),
        };
        tx.execute(
            "INSERT INTO track_lyrics (library_id, track_id, plain, synced) VALUES (?1, ?2, ?3, ?4)",
            params![lib, id, plain, synced],
        )?;
    }
    Ok(())
}
