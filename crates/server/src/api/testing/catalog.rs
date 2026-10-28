//! The catalog the tracks, albums, artists, and search tests share, and a walk through its
//! lists a page at a time.

use std::collections::HashSet;
use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use jewelcase_core::sort_key;
use rusqlite::{Result, Transaction};
use serde_json::Value;

use super::json;
use crate::db::Database;
use crate::db::testing::{Album, Artist, Image, Track, library};

/// [`super::app`] holding the catalog that `insert` describes.
pub async fn app() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = super::app("");
    db.write(insert).await.unwrap();
    (temp, db, app)
}

/// Adds two libraries. Library 1, "Music", holds:
///
/// - The Beatles (a photo whose placeholder could not be made, a biography, Rock and Pop),
///   Radiohead, Nirvana, and the unknown artist, added in the order Radiohead, the unknown
///   artist, The Beatles, Nirvana.
/// - "Abbey Road" by The Beatles (1969-09-26, Rock, on Apple, one disc of 17 tracks), "Kid A"
///   by Radiohead (2000, none of its tracks available), "Nevermind" by Nirvana (no date, and a
///   track total of 12 with no discs listed), and the unknown album by the unknown artist (an
///   unnumbered disc of 3 tracks). By date added they run Abbey Road, Nevermind, the unknown
///   album, Kid A.
/// - On Abbey Road, by The Beatles: "Come Together" (disc 1, track 1, Rock, explicit, with
///   synced lyrics, an ISRC, and loudness), "Something" (1-2, missing), "Her Majesty" (disc 1,
///   unnumbered), and "Bonus" (2-1, with Radiohead). On Kid A, by Radiohead with The Beatles:
///   "Idioteque" (1-8, missing). On the unknown album, by the unknown artist: "untitled" (no
///   disc, number, or date). Each track was added at its ID in milliseconds.
///
/// Library 2, "Other", holds an artist, an album, and a track of its own, each called
/// "Elsewhere". The albums' counts are set by hand, as for whole albums, of which the catalog
/// holds only some tracks, and so is The Beatles' genre Pop, which none of these tracks carries.
/// The artists', the tags' and the libraries' counts follow these rows. Abbey Road's discs list
/// only disc 1.
fn insert(tx: &Transaction) -> Result<()> {
    library(tx, 1, "Music", "/music")?;
    library(tx, 2, "Other", "/other")?;
    Image { placeholder: Some(Vec::new()), ..Image::new(40, 1, "The Beatles/artist.jpg") }
        .insert(tx)?;
    for (id, library, name, added_at) in [
        (10, 1, "The Beatles", 3000),
        (11, 1, "Radiohead", 1000),
        (12, 1, "Nirvana", 4000),
        (20, 2, "Elsewhere", 0),
    ] {
        Artist { added_at, ..Artist::new(id, library, name) }.insert(tx)?;
    }
    Artist { name: None, added_at: 2000, ..Artist::new(13, 1, "") }.insert(tx)?;

    let key = |name: &str| sort_key(name, None).into_bytes();
    let album = |id, library, title: Option<&str>, artist| Album {
        title: title.map(str::to_owned),
        sort_key: key(title.unwrap_or_default()),
        artist_sort_key: key(artist),
        disc_count: 1,
        ..Album::new(id, library)
    };
    Album {
        release_date: Some("1969-09-26".into()),
        track_count: 17,
        available_track_count: 17,
        duration_us: 2_830_000_000,
        loudness_lufs: Some(-9.5),
        peak_dbtp: Some(-0.3),
        added_at: 1000,
        ..album(101, 1, Some("Abbey Road"), "The Beatles")
    }
    .insert(tx)?;
    let kid_a = album(102, 1, Some("Kid A"), "Radiohead");
    Album { release_date: Some("2000".into()), track_count: 10, added_at: 4000, ..kid_a }
        .insert(tx)?;
    let nevermind = album(103, 1, Some("Nevermind"), "Nirvana");
    Album { track_count: 12, available_track_count: 12, added_at: 2000, ..nevermind }.insert(tx)?;
    let unknown = album(104, 1, None, "");
    Album { track_count: 3, available_track_count: 3, added_at: 3000, ..unknown }.insert(tx)?;
    let elsewhere = album(201, 2, Some("Elsewhere"), "Elsewhere");
    Album { track_count: 1, available_track_count: 1, ..elsewhere }.insert(tx)?;

    let track = |id, library, album, title: &str, artist, album_title| Track {
        title: title.to_owned(),
        sort_key: key(title),
        artist_sort_key: key(artist),
        album_sort_key: key(album_title),
        added_at: id,
        ..Track::new(id, library, album)
    };
    let abbey_road = |id, title, disc_number, track_number| Track {
        disc_number,
        track_number,
        release_date: Some("1969".into()),
        ..track(id, 1, 101, title, "The Beatles", "Abbey Road")
    };
    Track {
        file_size: 1000,
        bit_depth: Some(16),
        loudness_lufs: Some(-10.25),
        peak_dbtp: Some(-0.5),
        ..abbey_road(1001, "Come Together", 1, Some(1))
    }
    .insert(tx)?;
    Track { missing_since: Some(5), ..abbey_road(1002, "Something", 1, Some(2)) }.insert(tx)?;
    abbey_road(1003, "Her Majesty", 1, None).insert(tx)?;
    abbey_road(1004, "Bonus", 2, Some(1)).insert(tx)?;
    Track {
        disc_number: 1,
        track_number: Some(8),
        release_date: Some("2000".into()),
        missing_since: Some(5),
        ..track(1005, 1, 102, "Idioteque", "Radiohead", "Kid A")
    }
    .insert(tx)?;
    track(1006, 1, 104, "untitled", "", "").insert(tx)?;
    track(2001, 2, 201, "Elsewhere", "Elsewhere", "Elsewhere").insert(tx)?;

    tx.execute_batch(
        r#"INSERT INTO album_artists (library_id, album_id, position, artist_id) VALUES
               (1, 101, 0, 10), (1, 102, 0, 11), (1, 103, 0, 12), (1, 104, 0, 13), (2, 201, 0, 20);
           INSERT INTO track_artists (library_id, track_id, position, artist_id) VALUES
               (1, 1001, 0, 10), (1, 1002, 0, 10), (1, 1003, 0, 10), (1, 1004, 0, 10),
               (1, 1004, 1, 11), (1, 1005, 0, 11), (1, 1005, 1, 10), (1, 1006, 0, 13),
               (2, 2001, 0, 20);
           INSERT INTO album_discs (library_id, album_id, disc_number, track_count, track_total)
               VALUES (1, 101, 1, 17, 17), (1, 104, 0, 3, NULL);
           -- A tag of another type, which track 1001 brings to its album and artist, is never a
           -- genre.
           INSERT INTO tag_types (key) VALUES ('colour');
           INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES
               (30, 1, 'genre', 'Rock', 'rock', CAST('rock' AS BLOB)),
               (31, 1, 'genre', 'Pop', 'pop', CAST('pop' AS BLOB)),
               (32, 1, 'colour', 'Amber', 'amber', CAST('amber' AS BLOB));
           INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) VALUES
               (1, 1001, 30, 'rock'), (1, 1001, 32, 'Amber');
           INSERT INTO artist_tags (library_id, artist_id, tag_id, track_count) VALUES (1, 10, 31, 1);
           UPDATE artists SET image_id = 40, biography = 'Four lads from Liverpool.' WHERE id = 10;
           UPDATE albums SET disc_total = 1, track_total = 17, labels = '["Apple"]' WHERE id = 101;
           UPDATE albums SET track_total = 12 WHERE id = 103;
           UPDATE tracks SET explicit = 1, isrc = 'GBAYE0601690', lyrics_kind = 'synced',
                             bitrate_kbps = 900, identifiers = '{"musicbrainz_recordingid":"abc"}'
               WHERE id = 1001;
           UPDATE libraries SET track_count = 6, album_count = 4, artist_count = 4 WHERE id = 1;
           UPDATE libraries SET track_count = 1, album_count = 1, artist_count = 1 WHERE id = 2;"#,
    )
}

/// `field` of every item in the list at `uri`, read a page of `limit` at a time. Panics unless
/// every page but the last is full and carries a cursor not seen before, the last carries none,
/// and only an empty list has an empty page.
pub async fn page_through(app: &Router, uri: &str, limit: u32, field: &str) -> Value {
    let mut values = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen = HashSet::new();
    loop {
        let separator = if uri.contains('?') { '&' } else { '?' };
        let mut page_uri = format!("{uri}{separator}limit={limit}");
        if let Some(cursor) = &cursor {
            page_uri += &format!("&cursor={cursor}");
        }
        let (status, page) = json(app, &page_uri).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let items = page["items"].as_array().unwrap();
        assert!(cursor.is_none() || !items.is_empty(), "a cursor led to an empty page: {page_uri}");
        values.extend(items.iter().map(|item| item[field].clone()));
        match page["nextCursor"].as_str() {
            Some(next) => {
                assert_eq!(items.len(), limit as usize, "a short page has a cursor: {page_uri}");
                assert!(seen.insert(next.to_owned()), "a cursor came round again: {page_uri}");
                cursor = Some(next.to_owned());
            }
            None => {
                assert!(items.len() <= limit as usize, "{page_uri}");
                return values.into();
            }
        }
    }
}
