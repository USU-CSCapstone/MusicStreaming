use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::testing::{app, catalog, json, page_through};
use super::*;
use crate::api::page;
use crate::db::testing::{Album, Image, Track, library, scattered_key};

#[tokio::test]
async fn album_order_is_disc_then_number_with_unnumbered_tracks_last() {
    let (_temp, _db, app) = catalog::app().await;
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
    let (_temp, db, app) = catalog::app().await;
    db.write(|tx| {
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
async fn rows_alike_in_every_sort_column_page_by_id() {
    let (_temp, db, app) = app("");
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        Album::new(1, 1).insert(tx)?;
        // Tracks 1 to 4 share every sort value; 5 to 7 share having none to sort by.
        for id in 1..=7 {
            let known = |value: &str| if id <= 4 { value.as_bytes().to_vec() } else { Vec::new() };
            Track {
                sort_key: known("intro"),
                artist_sort_key: known("artist"),
                album_sort_key: known("album"),
                release_date: (id <= 4).then(|| "2000".into()),
                ..Track::new(id, 1, 1)
            }
            .insert(tx)?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let up = json!(["1", "2", "3", "4", "5", "6", "7"]);
    // Rows with no value to sort by come last in both orders; every track was added at once.
    let (down, newest) =
        (json!(["4", "3", "2", "1", "7", "6", "5"]), json!(["7", "6", "5", "4", "3", "2", "1"]));
    for limit in [1, 2, 3, 7] {
        for (sort, down) in [
            ("name", &down),
            ("artist", &down),
            ("album", &down),
            ("releaseDate", &down),
            ("dateAdded", &newest),
        ] {
            let uri = format!("/api/v1/libraries/1/tracks?sort={sort}");
            assert_eq!(page_through(&app, &uri, limit, "id").await, up, "{sort}, limit {limit}");
            let uri = format!("{uri}&order=desc");
            assert_eq!(page_through(&app, &uri, limit, "id").await, *down, "{sort}, limit {limit}");
        }
    }
}

#[tokio::test]
async fn filters_by_artist_credit() {
    let (_temp, _db, app) = catalog::app().await;
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
    let (_temp, _db, app) = catalog::app().await;
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

/// Identifiers that cannot be read are a server error rather than none. SQLite takes a number
/// too large for serde as JSON.
#[tokio::test]
async fn identifiers_that_cannot_be_read_are_an_error() {
    let (_temp, db, app) = catalog::app().await;
    let unreadable = "UPDATE tracks SET identifiers = '{\"musicbrainz_recordingid\": 1e400}'";
    db.write(move |tx| tx.execute(unreadable, [])).await.unwrap();
    let (status, problem) = json(&app, "/api/v1/libraries/1/tracks/1001").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{problem}");
}

#[tokio::test]
async fn images_carry_their_placeholders() {
    let (_temp, db, app) = catalog::app().await;
    // Abbey Road's cover and the photo of The Beatles each have a placeholder of their own.
    db.write(|tx| {
        Image { placeholder: Some(vec![0x00, 0xff, 0x10]), ..Image::new(41, 1, "cover.jpg") }
            .insert(tx)?;
        tx.execute_batch(
            "UPDATE albums SET image_id = 41 WHERE id = 101;
             UPDATE images SET placeholder = x'102030' WHERE id = 40;",
        )
    })
    .await
    .unwrap();
    let cover = json!({ "id": "41", "placeholder": "AP8Q" });
    let (_, track) = json(&app, "/api/v1/libraries/1/tracks/1001").await;
    assert_eq!(track["album"]["image"], cover);
    let (_, album) = json(&app, "/api/v1/libraries/1/albums/101").await;
    assert_eq!(album["image"], cover);
    let (_, artist) = json(&app, "/api/v1/libraries/1/artists/10").await;
    assert_eq!(artist["image"], json!({ "id": "40", "placeholder": "ECAw" }));
}

#[tokio::test]
async fn another_librarys_track_is_not_found() {
    let (_temp, _db, app) = catalog::app().await;
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
    let (_temp, _db, app) = catalog::app().await;
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

/// A track row reads its album once, for its title, its image and the image's placeholder alike.
#[tokio::test]
async fn a_track_row_reads_its_album_once() {
    let (_temp, db, _app) = catalog::app().await;
    db.read(|conn| {
        let source = source(1, None, None, ArtistCredit::Any);
        let sql = format!(
            "EXPLAIN QUERY PLAN SELECT {} FROM {} {} WHERE {}",
            SELECT.join(", "),
            source.from,
            source.joins,
            source.filter
        );
        let plan: Vec<String> = conn
            .prepare(&sql)?
            .query_map(rusqlite::params_from_iter(&source.params), |row| row.get(3))?
            .collect::<rusqlite::Result<_>>()?;
        let reads = plan
            .iter()
            .filter(|step| step.starts_with("SEARCH al ") || step.starts_with("SEARCH albums "));
        assert_eq!(reads.count(), 1, "{plan:#?}");
        Ok(())
    })
    .await
    .unwrap();
}
