use std::path::Path;

use axum::http::StatusCode;
use rusqlite::Transaction;
use serde_json::{Value, json};

use super::super::testing::{app, json, page_through};
use super::*;
use crate::api::page;
use crate::db::libraries;
use crate::db::testing::{Album, Image, Track, library, scattered_key};

/// Library 1: "Abbey Road" by The Beatles with discs 1 and 2, one disc-1 track unnumbered,
/// and one track missing; "Kid A" by Radiohead with a track The Beatles feature on; and
/// an untagged file on the unknown album. Library 2 holds one track of its own.
pub(in crate::api) fn fixture(tx: &Transaction) -> rusqlite::Result<()> {
    libraries::create(tx, Some(1), "Music", &[Path::new("/music")], &[])?;
    libraries::create(tx, Some(2), "Other", &[Path::new("/other")], &[])?;
    let root = |library: i64| -> rusqlite::Result<i64> {
        tx.query_row("SELECT id FROM library_roots WHERE library_id = ?1", [library], |r| r.get(0))
    };
    let (music, other) = (root(1)?, root(2)?);
    tx.execute_batch(
        "INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at, updated_at) VALUES
             (10, 1, 'The Beatles', 'the beatles', CAST('beatles' AS BLOB), 0, 0),
             (11, 1, 'Radiohead', 'radiohead', CAST('radiohead' AS BLOB), 0, 0),
             (13, 1, NULL, '', x'', 0, 0),
             (20, 2, 'Elsewhere', 'elsewhere', CAST('elsewhere' AS BLOB), 0, 0);
         INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key,
                             artist_sort_key, loudness_lufs, peak_dbtp, added_at, updated_at) VALUES
             (101, 1, 'Abbey Road', 'abbey road', 'the beatles', CAST('abbey road' AS BLOB),
              CAST('beatles' AS BLOB), -9.5, -0.3, 0, 0),
             (102, 1, 'Kid A', 'kid a', 'radiohead', CAST('kid a' AS BLOB),
              CAST('radiohead' AS BLOB), NULL, NULL, 0, 0),
             (104, 1, NULL, '', '', x'', x'', NULL, NULL, 0, 0),
             (201, 2, 'Elsewhere', 'elsewhere', 'elsewhere', CAST('elsewhere' AS BLOB),
              CAST('elsewhere' AS BLOB), NULL, NULL, 0, 0);
         INSERT INTO album_artists (library_id, album_id, position, artist_id) VALUES
             (1, 101, 0, 10), (1, 102, 0, 11), (1, 104, 0, 13), (2, 201, 0, 20);
         INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES
             (30, 1, 'genre', 'Rock', 'rock', CAST('rock' AS BLOB));",
    )?;
    let track = "INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size,
                     file_mtime, title, sort_key, artist_sort_key, album_sort_key, disc_number,
                     track_number, release_date, missing_since, codec, container, lossless,
                     sample_rate_hz, bit_depth, bitrate_kbps, channels, duration_us, added_at,
                     updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1000, 0, ?6, CAST(lower(?6) AS BLOB), ?7, ?8, ?9,
                         ?10, ?11, ?12, 'flac', 'flac', 1, 44100, 16, 900, 2, 1000000, ?1, 0)";
    let beatles = rusqlite::types::Value::Blob(b"beatles".to_vec());
    let radiohead = rusqlite::types::Value::Blob(b"radiohead".to_vec());
    let empty = rusqlite::types::Value::Blob(Vec::new());
    let abbey = rusqlite::types::Value::Blob(b"abbey road".to_vec());
    let kid_a = rusqlite::types::Value::Blob(b"kid a".to_vec());
    let null = rusqlite::types::Value::Null;
    use rusqlite::types::Value::{Integer, Text};
    for row in [
        // id, library, album, root, path, title, artist key, album key, disc, number, date, missing
        vec![
            Integer(1001),
            Integer(1),
            Integer(101),
            Integer(music),
            Text("1.flac".into()),
            Text("Come Together".into()),
            beatles.clone(),
            abbey.clone(),
            Integer(1),
            Integer(1),
            Text("1969".into()),
            null.clone(),
        ],
        vec![
            Integer(1002),
            Integer(1),
            Integer(101),
            Integer(music),
            Text("2.flac".into()),
            Text("Something".into()),
            beatles.clone(),
            abbey.clone(),
            Integer(1),
            Integer(2),
            Text("1969".into()),
            Integer(5),
        ],
        vec![
            Integer(1003),
            Integer(1),
            Integer(101),
            Integer(music),
            Text("x.flac".into()),
            Text("Her Majesty".into()),
            beatles.clone(),
            abbey.clone(),
            Integer(1),
            null.clone(),
            Text("1969".into()),
            null.clone(),
        ],
        vec![
            Integer(1004),
            Integer(1),
            Integer(101),
            Integer(music),
            Text("d2.flac".into()),
            Text("Bonus".into()),
            beatles.clone(),
            abbey.clone(),
            Integer(2),
            Integer(1),
            Text("1969".into()),
            null.clone(),
        ],
        vec![
            Integer(1005),
            Integer(1),
            Integer(102),
            Integer(music),
            Text("k.flac".into()),
            Text("Idioteque".into()),
            radiohead.clone(),
            kid_a.clone(),
            Integer(1),
            Integer(8),
            Text("2000".into()),
            null.clone(),
        ],
        vec![
            Integer(1006),
            Integer(1),
            Integer(104),
            Integer(music),
            Text("u.flac".into()),
            Text("untitled".into()),
            empty.clone(),
            empty.clone(),
            Integer(0),
            null.clone(),
            null.clone(),
            null.clone(),
        ],
        vec![
            Integer(2001),
            Integer(2),
            Integer(201),
            Integer(other),
            Text("e.flac".into()),
            Text("Elsewhere".into()),
            empty.clone(),
            empty.clone(),
            Integer(0),
            null.clone(),
            null.clone(),
            null.clone(),
        ],
    ] {
        tx.execute(track, rusqlite::params_from_iter(row))?;
    }
    tx.execute_batch(
        "INSERT INTO track_artists (library_id, track_id, position, artist_id) VALUES
             (1, 1001, 0, 10), (1, 1002, 0, 10), (1, 1003, 0, 10), (1, 1004, 0, 10),
             (1, 1005, 0, 11), (1, 1005, 1, 10), (1, 1006, 0, 13), (2, 2001, 0, 20);
         INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) VALUES (1, 1001, 30, 'rock');
         UPDATE tracks SET explicit = 1, isrc = 'GBAYE0601690', lyrics_kind = 'synced',
                           loudness_lufs = -10.25, peak_dbtp = -0.5,
                           identifiers = '{\"musicbrainz_recordingid\":\"abc\"}' WHERE id = 1001;
         UPDATE libraries SET track_count = 6 WHERE id = 1;",
    )
}

async fn app_with_fixture() -> (tempfile::TempDir, axum::Router) {
    let (temp, db, app) = app("");
    db.write(fixture).await.unwrap();
    (temp, app)
}

#[tokio::test]
async fn album_order_is_disc_then_number_with_unnumbered_tracks_last() {
    let (_temp, app) = app_with_fixture().await;
    let base = "/api/v1/libraries/1/tracks?sort=album";
    for limit in [1, 2, 100] {
        assert_eq!(
            page_through(&app, base, limit, "title").await,
            json!(["Come Together", "Something", "Her Majesty", "Bonus", "Idioteque", "untitled"]),
            "limit {limit}"
        );
        assert_eq!(
            page_through(&app, &format!("{base}&albumId=101"), limit, "title").await,
            json!(["Come Together", "Something", "Her Majesty", "Bonus"]),
            "limit {limit}"
        );
        assert_eq!(
            page_through(
                &app,
                "/api/v1/libraries/1/tracks?sort=releaseDate&order=desc",
                limit,
                "title"
            )
            .await,
            json!(["Idioteque", "Bonus", "Her Majesty", "Something", "Come Together", "untitled"]),
            "limit {limit}"
        );
        assert_eq!(
            page_through(&app, "/api/v1/libraries/1/tracks", limit, "title").await,
            json!(["Bonus", "Come Together", "Her Majesty", "Idioteque", "Something", "untitled"]),
            "limit {limit}"
        );
    }
    let (_, page) = json(&app, "/api/v1/libraries/1/tracks").await;
    assert_eq!(page["total"], 6);
    let (_, page) = json(&app, &format!("{base}&albumId=101")).await;
    assert_eq!(page["total"], 4);
}

#[tokio::test]
async fn duration_order_breaks_ties_by_id() {
    let (_temp, db, app) = app("");
    db.write(|tx| {
        fixture(tx)?;
        tx.execute_batch(
            "UPDATE tracks SET duration_us = CASE id
                 WHEN 1001 THEN 259000000 WHEN 1002 THEN 182000000 WHEN 1003 THEN 23000000
                 WHEN 1004 THEN 182000000 WHEN 1005 THEN 251000000 ELSE duration_us END;",
        )
    })
    .await
    .unwrap();
    let base = "/api/v1/libraries/1/tracks?sort=duration";
    for limit in [1, 2, 100] {
        assert_eq!(
            page_through(&app, base, limit, "title").await,
            json!(["untitled", "Her Majesty", "Something", "Bonus", "Idioteque", "Come Together"]),
            "limit {limit}"
        );
        assert_eq!(
            page_through(&app, &format!("{base}&order=desc"), limit, "title").await,
            json!(["Come Together", "Idioteque", "Bonus", "Something", "Her Majesty", "untitled"]),
            "limit {limit}"
        );
    }
}

#[tokio::test]
async fn filters_by_artist_credit() {
    let (_temp, app) = app_with_fixture().await;
    let base = "/api/v1/libraries/1/tracks?sort=album";
    for (query, expected) in [
        (
            "artistId=10&artistCredit=owned",
            json!(["Come Together", "Something", "Her Majesty", "Bonus"]),
        ),
        ("artistId=10&artistCredit=featured", json!(["Idioteque"])),
        ("artistId=10", json!(["Come Together", "Something", "Her Majesty", "Bonus", "Idioteque"])),
        ("artistId=10&albumId=102", json!(["Idioteque"])),
        ("artistId=20", json!([])),
        ("albumId=nope", json!([])),
    ] {
        assert_eq!(
            page_through(&app, &format!("{base}&{query}"), 2, "title").await,
            expected,
            "{query}"
        );
    }
}

#[tokio::test]
async fn summaries_and_full_tracks() {
    let (_temp, app) = app_with_fixture().await;
    let summary = json!({
        "id": "1001", "title": "Come Together",
        "artists": [{ "id": "10", "name": "The Beatles" }],
        "album": { "id": "101", "title": "Abbey Road",
                   "artists": [{ "id": "10", "name": "The Beatles" }], "image": null },
        "discNumber": 1, "trackNumber": 1, "durationUs": 1_000_000, "releaseDate": "1969",
        "genres": [{ "id": "30", "name": "Rock" }], "explicit": true,
        "audio": { "codec": "flac", "lossless": true }, "availability": "available",
        "addedAt": "1970-01-01T00:00:01.001Z",
        "sortKeys": { "title": "come together", "artist": "beatles", "album": "abbey road" },
    });
    let (_, page) = json(&app, "/api/v1/libraries/1/tracks?albumId=101&sort=album").await;
    assert_eq!(page["items"][0], summary);
    assert_eq!(page["items"][1]["availability"], "missing");
    assert_eq!(page["items"][2]["trackNumber"], Value::Null);

    let (status, track) = json(&app, "/api/v1/libraries/1/tracks/1001").await;
    assert_eq!(status, StatusCode::OK);
    let mut expected = summary;
    expected["audio"] = json!({
        "codec": "flac", "lossless": true, "container": "flac", "bitrateKbps": 900,
        "sampleRateHz": 44100, "bitDepth": 16, "channels": 2, "fileSizeBytes": 1000,
    });
    expected["lyrics"] = json!("synced");
    expected["loudness"] = json!({ "trackLufs": -10.25, "trackPeakDbtp": -0.5,
                                   "albumLufs": -9.5, "albumPeakDbtp": -0.3 });
    expected["identifiers"] = json!({ "isrc": "GBAYE0601690", "musicbrainz_recordingid": "abc" });
    assert_eq!(track, expected);

    let (_, untagged) = json(&app, "/api/v1/libraries/1/tracks/1006").await;
    assert_eq!(untagged["discNumber"], Value::Null);
    assert_eq!(untagged["album"]["title"], Value::Null);
    assert_eq!(untagged["loudness"], Value::Null);
    assert_eq!(untagged["identifiers"], json!({ "isrc": null }));
    assert_eq!(untagged["sortKeys"], json!({ "title": "untitled", "artist": "", "album": "" }));
}

#[tokio::test]
async fn images_carry_their_placeholders() {
    let (_temp, db, app) = app("");
    db.write(|tx| {
        fixture(tx)?;
        // Abbey Road's cover has a placeholder; The Beatles' photo could not be decoded, which
        // the placeholder job records as empty.
        Image { placeholder: Some(vec![0x00, 0xff, 0x10]), ..Image::new(40, 1, "cover.jpg") }
            .insert(tx)?;
        Image { placeholder: Some(Vec::new()), ..Image::new(41, 1, "artist.jpg") }.insert(tx)?;
        tx.execute_batch(
            "UPDATE albums SET image_id = 40 WHERE id = 101;
             UPDATE artists SET image_id = 41 WHERE id = 10;",
        )
    })
    .await
    .unwrap();
    let cover = json!({ "id": "40", "placeholder": "AP8Q" });
    let (_, track) = json(&app, "/api/v1/libraries/1/tracks/1001").await;
    assert_eq!(track["album"]["image"], cover);
    let (_, album) = json(&app, "/api/v1/libraries/1/albums/101").await;
    assert_eq!(album["image"], cover);
    let (_, artist) = json(&app, "/api/v1/libraries/1/artists/10").await;
    assert_eq!(artist["image"], json!({ "id": "41", "placeholder": "" }));
}

#[tokio::test]
async fn another_librarys_track_is_not_found() {
    let (_temp, app) = app_with_fixture().await;
    for uri in [
        "/api/v1/libraries/1/tracks/2001",
        "/api/v1/libraries/3/tracks/1001",
        "/api/v1/libraries/3/tracks",
        "/api/v1/libraries/1/tracks/-1",
    ] {
        let (status, problem) = json(&app, uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(problem["code"], "not_found", "{uri}");
    }
}

#[tokio::test]
async fn bad_parameters_are_validation_problems() {
    let (_temp, app) = app_with_fixture().await;
    let base = "/api/v1/libraries/1/tracks";
    let (_, page) = json(&app, &format!("{base}?sort=album&limit=1")).await;
    let album_cursor = page["nextCursor"].as_str().unwrap().to_owned();
    for query in [
        "limit=1001".to_owned(),
        "sort=top".to_owned(),
        "genre=30".to_owned(),
        format!("sort=releaseDate&cursor={album_cursor}"),
        format!("cursor={album_cursor}"),
    ] {
        let (status, problem) = json(&app, &format!("{base}?{query}")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
        assert_eq!(problem["code"], "validation_failed", "{query}");
    }
}

#[tokio::test]
async fn every_sort_reads_from_an_index() {
    let (_temp, db, _app) = app("");
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        // 20,000 tracks on 2,000 albums: a sixth undated, some unnumbered, a few with
        // unknown names.
        let album_key = |i| if i % 200 == 0 { Vec::new() } else { scattered_key("album", i) };
        for i in 1..=2000 {
            Album { sort_key: album_key(i), added_at: i, ..Album::new(i, 1) }.insert(tx)?;
        }
        for i in 1..=20_000 {
            let album = 1 + i % 2000;
            let artist_key = format!("artist {:05}", i % 3000).into_bytes();
            let track = Track {
                path: format!("p{i}"),
                sort_key: scattered_key("track", i),
                artist_sort_key: if i % 997 == 0 { Vec::new() } else { artist_key },
                album_sort_key: album_key(album),
                disc_number: 1 + i % 2,
                track_number: (i % 13 != 0).then_some(i % 12),
                release_date: (i % 6 != 0).then(|| format!("{:04}", 1950 + i % 75)),
                duration_us: (i * 7919) % 600_000_000,
                added_at: i,
                ..Track::new(i, 1, album)
            };
            track.insert(tx)?;
        }
        tx.execute_batch("ANALYZE")
    })
    .await
    .unwrap();
    db.read(|conn| {
        for sort in [
            TrackSort::Name,
            TrackSort::Artist,
            TrackSort::Album,
            TrackSort::DateAdded,
            TrackSort::ReleaseDate,
            TrackSort::Duration,
        ] {
            let source = source(1, None, None, ArtistCredit::Any);
            page::assert_indexed(conn, &source, sort.sort())?;
        }
        let one_album = source(1, Some("7"), None, ArtistCredit::Any);
        page::assert_indexed(conn, &one_album, &WITHIN_ALBUM)?;
        Ok(())
    })
    .await
    .unwrap();
}
