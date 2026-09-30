//! Writing one scanned file: its track row, credits, genres, lyrics, and artwork, in the batch's
//! transaction.

use jewelcase_core::fold;
use jewelcase_scanner::{ArtworkSource, TrackRecord};
use rusqlite::{Result, Transaction, params};

use super::entities::{
    find_or_create_album, find_or_create_artist, find_or_create_tag, upsert_image,
};
use super::roots::{Roots, relative};
use crate::db::catalog::Touched;
use crate::db::feed::{self, Entity, Op};

pub fn upsert_track(
    roots: &Roots,
    tx: &Transaction<'_>,
    lib: i64,
    rec: &TrackRecord,
    scan_id: i64,
    now: i64,
    touched: &mut Touched,
) -> Result<()> {
    let Some(root_id) = roots.root_by_path(lib, &rec.root) else {
        tracing::warn!(path = %rec.path.display(), "track under an unknown root; skipped");
        return Ok(());
    };
    let rel = relative(&rec.root, &rec.path);
    if let Some(id) = rec.id {
        // Whatever it pointed at before may now be orphaned.
        touched.track(tx, id as i64)?;
    }

    // Artists: the unknown artist (key "") when a file names none.
    let credits: Vec<Option<&str>> = if rec.tags.artists.is_empty() {
        vec![None]
    } else {
        rec.tags.artists.iter().map(|s| Some(s.as_str())).collect()
    };
    let mut artist_ids = Vec::new();
    for name in &credits {
        artist_ids.push(find_or_create_artist(tx, lib, *name, now)?);
    }

    // Album artists: tagged, else the track artists, else unknown for a
    // compilation with no album-artist tag (`requirements/scanning.md` §2).
    let album_credits: Vec<Option<&str>> = if !rec.tags.album_artists.is_empty() {
        rec.tags
            .album_artists
            .iter()
            .map(|s| Some(s.as_str()))
            .collect()
    } else if rec.tags.compilation {
        vec![None]
    } else {
        credits.clone()
    };
    let mut album_artist_ids = Vec::new();
    for name in &album_credits {
        album_artist_ids.push(find_or_create_artist(tx, lib, *name, now)?);
    }
    let artists_key = fold::artists_key(album_credits.iter().map(|n| n.unwrap_or("")));
    let album_id = find_or_create_album(
        tx,
        lib,
        rec.tags.album.as_deref(),
        &artists_key,
        &rec.sort.album,
        &rec.sort.album_artist,
        now,
    )?;

    let identifiers = {
        let mut m = serde_json::Map::new();
        if let Some(v) = &rec.tags.musicbrainz_recording_id {
            m.insert("musicbrainz_recordingid".into(), v.clone().into());
        }
        if let Some(v) = &rec.tags.musicbrainz_release_id {
            m.insert("musicbrainz_albumid".into(), v.clone().into());
        }
        serde_json::Value::Object(m).to_string()
    };
    let lyrics = rec.lyrics();
    let lyrics_kind = match lyrics {
        None => "none",
        Some(l) if l.synced => "synced",
        Some(_) => "plain",
    };
    let format = rec.properties.format;

    // A new track (no id yet) gets a random one.
    let id: i64 = tx.query_row(
            "INSERT INTO tracks (id, library_id, album_id, album_title, fingerprint, root_id, path, file_size, file_mtime, \
             missing_since, last_seen_scan_id, title, sort_key, artist_sort_key, album_sort_key, disc_number, track_number, \
             track_total, disc_total, release_date, explicit, compilation, release_type, isrc, identifiers, lyrics_kind, \
             codec, container, lossless, bitrate_kbps, sample_rate_hz, bit_depth, channels, duration_us, added_at, updated_at) \
             VALUES (coalesce(?1, random() & 0x7FFFFFFFFFFFFFFF), ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8, NULL, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, \
             ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?33) \
             ON CONFLICT (id) DO UPDATE SET album_id = excluded.album_id, album_title = excluded.album_title, \
             root_id = excluded.root_id, path = excluded.path, file_size = excluded.file_size, file_mtime = excluded.file_mtime, \
             missing_since = NULL, last_seen_scan_id = excluded.last_seen_scan_id, title = excluded.title, \
             sort_key = excluded.sort_key, artist_sort_key = excluded.artist_sort_key, album_sort_key = excluded.album_sort_key, \
             disc_number = excluded.disc_number, track_number = excluded.track_number, track_total = excluded.track_total, \
             disc_total = excluded.disc_total, release_date = excluded.release_date, explicit = excluded.explicit, \
             compilation = excluded.compilation, release_type = excluded.release_type, isrc = excluded.isrc, \
             identifiers = excluded.identifiers, lyrics_kind = excluded.lyrics_kind, codec = excluded.codec, \
             container = excluded.container, lossless = excluded.lossless, bitrate_kbps = excluded.bitrate_kbps, \
             sample_rate_hz = excluded.sample_rate_hz, bit_depth = excluded.bit_depth, channels = excluded.channels, \
             duration_us = excluded.duration_us, updated_at = excluded.updated_at \
             RETURNING id",
            params![
                rec.id.map(|id| id as i64),
                lib,
                album_id,
                rec.tags.album,
                root_id,
                rel,
                rec.size as i64,
                rec.mtime_ms as i64,
                scan_id,
                rec.display_title(),
                rec.sort.title.as_bytes(),
                rec.sort.artist.as_bytes(),
                rec.sort.album.as_bytes(),
                rec.tags.disc_number.unwrap_or(0) as i64,
                rec.tags.track_number.map(|n| n as i64),
                rec.tags.track_total.map(|n| n as i64),
                rec.tags.disc_total.map(|n| n as i64),
                rec.tags.release_date.map(|d| d.to_string()),
                rec.tags.explicit == Some(true),
                rec.tags.compilation,
                rec.tags.release_type,
                rec.tags.isrc,
                identifiers,
                lyrics_kind,
                format.codec_name(),
                format.container_name(),
                format.is_lossless(),
                rec.properties.bitrate_kbps.map(|b| b as i64),
                rec.properties.sample_rate.unwrap_or(0) as i64,
                rec.properties.bit_depth.map(|b| b as i64),
                rec.properties.channels.unwrap_or(0) as i64,
                (rec.properties.duration_ms * 1000) as i64,
                now,
            ],
            |r| r.get(0),
        )?;

    // Links: replace wholesale.
    tx.execute("DELETE FROM track_artists WHERE track_id = ?1", [id])?;
    tx.execute("DELETE FROM track_album_artists WHERE track_id = ?1", [id])?;
    tx.execute("DELETE FROM track_tags WHERE track_id = ?1", [id])?;
    for (pos, (artist_id, name)) in artist_ids.iter().zip(&credits).enumerate() {
        tx.execute(
                "INSERT OR IGNORE INTO track_artists (library_id, track_id, position, artist_id, artist_name) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![lib, id, pos as i64, artist_id, name.map(str::trim)],
            )?;
    }
    for (pos, (artist_id, name)) in album_artist_ids.iter().zip(&album_credits).enumerate() {
        tx.execute(
                "INSERT OR IGNORE INTO track_album_artists (library_id, track_id, position, artist_id, artist_name) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![lib, id, pos as i64, artist_id, name.map(str::trim)],
            )?;
    }
    for genre in &rec.tags.genres {
        let tag_id = find_or_create_tag(tx, lib, genre)?;
        tx.execute(
                "INSERT OR IGNORE INTO track_tags (library_id, track_id, tag_id, tag_name) VALUES (?1, ?2, ?3, ?4)",
                params![lib, id, tag_id, genre.trim()],
            )?;
        touched.tags.insert(tag_id);
    }

    // Lyrics.
    tx.execute("DELETE FROM track_lyrics WHERE track_id = ?1", [id])?;
    if let Some(l) = lyrics {
        let synced = if l.synced {
            let lines = jewelcase_core::lrc::parse(&l.text);
            if lines.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&lines).unwrap())
            }
        } else {
            None
        };
        tx.execute(
                "INSERT INTO track_lyrics (library_id, track_id, plain, synced, embedded) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![lib, id, l.text, synced, rec.tags.lyrics.is_some()],
            )?;
    }

    // Artwork: an images row per distinct source; the album keeps the
    // first it is given (`requirements/scanning.md` §3.1).
    if let Some(art) = &rec.artwork {
        let (img_path, embedded) = match &art.source {
            ArtworkSource::Embedded => (rel.clone(), true),
            ArtworkSource::Sidecar(p) => (relative(&rec.root, p), false),
        };
        let image_id = upsert_image(tx, lib, root_id, &img_path, embedded, &art.info)?;
        tx.execute(
            "UPDATE albums SET image_id = ?2 WHERE id = ?1 AND image_id IS NULL",
            params![album_id, image_id],
        )?;
    }
    let artist_targets: &[i64] = if album_credits.iter().any(|c| c.is_some()) {
        &album_artist_ids
    } else {
        &artist_ids
    };
    if let Some(img) = &rec.artist_image {
        let image_id = upsert_image(
            tx,
            lib,
            root_id,
            &relative(&rec.root, &img.path),
            false,
            &img.info,
        )?;
        for artist_id in artist_targets {
            tx.execute(
                    "UPDATE artists SET image_id = ?2 WHERE id = ?1 AND image_id IS NULL AND name IS NOT NULL",
                    params![artist_id, image_id],
                )?;
        }
    }
    if let Some(bio) = &rec.artist_biography {
        for artist_id in artist_targets {
            tx.execute(
                    "UPDATE artists SET biography = ?2 WHERE id = ?1 AND biography IS NULL AND name IS NOT NULL",
                    params![artist_id, bio.text],
                )?;
        }
    }

    touched.albums.insert(album_id);
    touched
        .artists
        .extend(artist_ids.iter().chain(&album_artist_ids));
    feed::record(tx, lib, Entity::Track, id, Op::Upsert, now)?;
    Ok(())
}
