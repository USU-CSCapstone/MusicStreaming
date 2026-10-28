use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::testing::catalog::{self, page_through};
use super::super::testing::{app, json};
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

/// An artist filter's total is the artist's owned and featured tracks, counted from its album
/// credits; an album filter, alone or with an artist, counts its rows.
#[tokio::test]
async fn an_artist_filters_total_is_its_credited_tracks() {
    let (_temp, db, app) = catalog::app().await;
    // A second track of Kid A that credits The Beatles makes two featured tracks on one featured
    // album.
    db.write(|tx| {
        Track::new(1007, 1, 102).insert(tx)?;
        tx.execute_batch(
            "INSERT INTO track_artists (library_id, track_id, position, artist_id) \
             VALUES (1, 1007, 0, 11), (1, 1007, 1, 10)",
        )
    })
    .await
    .unwrap();
    /// The `total` of library 1's track list for `query`.
    async fn total(app: &axum::Router, query: &str) -> Value {
        json(app, &format!("/api/v1/libraries/1/tracks?{query}")).await.1["total"].clone()
    }
    for (query, expected) in [
        ("artistId=10&artistCredit=owned", 4),
        ("artistId=10&artistCredit=featured", 2),
        ("artistId=10", 6),
        ("artistId=10&albumId=102", 2),
        ("albumId=101", 4),
        ("artistId=20", 0),
        ("artistId=nobody", 0),
    ] {
        assert_eq!(total(&app, query).await, expected, "{query}");
    }
    let (_, albums) =
        json(&app, "/api/v1/libraries/1/albums?artistId=10&artistCredit=featured").await;
    assert_eq!(albums["total"], 1, "two tracks on one album are one album");
    let (_, artist) = json(&app, "/api/v1/libraries/1/artists/10").await;
    assert_eq!(artist["appearanceCount"], 2);
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
    // Its ISRC and other identifiers are stored for plugins, never served
    // (`requirements/general.md` §3.7).
    assert_eq!(track, expected);

    let (_, untagged) = json(&app, "/api/v1/libraries/1/tracks/1006").await;
    assert_eq!(untagged["discNumber"], Value::Null);
    assert_eq!(untagged["album"]["title"], Value::Null);
    assert_eq!(untagged["loudness"], Value::Null);
    assert_eq!(untagged["sortKeys"], json!({ "title": "untitled", "artist": "", "album": "" }));
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

/// An image's placeholder is `null` both before the placeholder job has made one and once it
/// has found the image cannot be decoded.
#[tokio::test]
async fn an_image_without_a_placeholder_has_null_for_one() {
    let (_temp, db, app) = catalog::app().await;
    // Abbey Road's cover has none made yet; the photo of The Beatles could not be decoded.
    db.write(|tx| {
        Image::new(41, 1, "cover.jpg").insert(tx)?;
        tx.execute("UPDATE albums SET image_id = 41 WHERE id = 101", [])
    })
    .await
    .unwrap();
    let (_, track) = json(&app, "/api/v1/libraries/1/tracks/1001").await;
    assert_eq!(track["album"]["image"], json!({ "id": "41", "placeholder": null }));
    let (_, artist) = json(&app, "/api/v1/libraries/1/artists/10").await;
    assert_eq!(artist["image"], json!({ "id": "40", "placeholder": null }));
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
        tx.execute_batch(
            "INSERT INTO tags (id, library_id, type, name, name_key, sort_key) \
             VALUES (1, 1, 'genre', 'Half', 'half', x'68'); \
             INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) \
             SELECT 1, id, 1, 'Half' FROM tracks WHERE id % 2 = 1; \
             WITH RECURSIVE n (i) AS (SELECT 2 UNION ALL SELECT i + 1 FROM n WHERE i < 300) \
             INSERT INTO tags (id, library_id, type, name, name_key, sort_key) \
             SELECT i, 1, 'genre', 'Genre ' || i, 'genre ' || i, CAST('genre ' || i AS BLOB) FROM n; \
             INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) \
             SELECT 1, id, 2 + id % 299, 'Genre' FROM tracks WHERE id % 2 = 0;",
        )?;
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
            let source = source(conn, 1, None, None, ArtistCredit::Any, &[], TagMatch::Any)?;
            page::assert_indexed(conn, &source, sort.sort())?;
        }
        let one_album =
            source(conn, 1, Some(AnyId(Some(7))), None, ArtistCredit::Any, &[], TagMatch::Any)?;
        page::assert_indexed(conn, &one_album, &WITHIN_ALBUM)?;
        // Half the tracks carry tag 1, too many to lead, so a page walks each sort's index. The
        // other half carry one of 299 other genres, so the planner knows tags as a library has them.
        let tagged = [AnyId(Some(1))];
        for sort in [TrackSort::Name, TrackSort::DateAdded, TrackSort::ReleaseDate] {
            let source = source(conn, 1, None, None, ArtistCredit::Any, &tagged, TagMatch::Any)?;
            assert!(matches!(source.total, page::Total::Stored(10_000)));
            page::assert_indexed(conn, &source, sort.sort())?;
        }
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
        let source = source(conn, 1, None, None, ArtistCredit::Any, &[], TagMatch::Any)?;
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

/// 300 tracks of library 1, `0001` to `0300` by name, each on album `1 + i % 10`, every fourth
/// credited to artist 9. Genre Rock (30) is on every third, Jazz (31) every fifth, Blues (34)
/// every seventh, mood Calm (32) every second, and mood Rare (33) on track 15 alone. Library 2 has a track of its own, which
/// its Rock (50) is on.
async fn tagged_library() -> (tempfile::TempDir, std::sync::Arc<crate::db::Database>, axum::Router)
{
    let (temp, db, app) = app("");
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        library(tx, 2, "Other", "/other")?;
        for album in 1..=10 {
            Album::new(album, 1).insert(tx)?;
        }
        Album::new(20, 2).insert(tx)?;
        for i in 1..=300 {
            let sort_key = format!("{i:04}").into_bytes();
            Track { sort_key, added_at: i, ..Track::new(i, 1, 1 + i % 10) }.insert(tx)?;
        }
        Track::new(400, 2, 20).insert(tx)?;
        tx.execute_batch(
            "INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at)
             VALUES (9, 1, 'Nine', 'nine', x'6e', 0);
             INSERT INTO track_artists (library_id, track_id, position, artist_id, artist_name)
             SELECT 1, id, 0, 9, 'Nine' FROM tracks WHERE library_id = 1 AND id % 4 = 0;
             INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES
                 (30, 1, 'genre', 'Rock', 'rock', x'72'), (31, 1, 'genre', 'Jazz', 'jazz', x'6a'),
                 (32, 1, 'mood', 'Calm', 'calm', x'63'), (33, 1, 'mood', 'Rare', 'rare', x'72'),
                 (34, 1, 'genre', 'Blues', 'blues', x'62'),
                 (50, 2, 'genre', 'Rock', 'rock', x'72');
             INSERT INTO track_tags (library_id, track_id, tag_id, tag_name)
             SELECT 1, id, 30, 'Rock' FROM tracks WHERE library_id = 1 AND id % 3 = 0
             UNION ALL SELECT 1, id, 31, 'Jazz' FROM tracks WHERE library_id = 1 AND id % 5 = 0
             UNION ALL SELECT 1, id, 32, 'Calm' FROM tracks WHERE library_id = 1 AND id % 2 = 0
             UNION ALL SELECT 1, id, 34, 'Blues' FROM tracks WHERE library_id = 1 AND id % 7 = 0
             UNION ALL SELECT 1, 15, 33, 'Rare'
             UNION ALL SELECT 2, 400, 50, 'Rock';",
        )
    })
    .await
    .unwrap();
    (temp, db, app)
}

/// Which of library 1's tracks, by ID, a query lists.
type Keep = fn(i64) -> bool;

/// Each query's tracks are those of library 1 that its [`Keep`] keeps, in name order, whether a
/// page starts from a tag (Rare, one track of 300) or walks the sort's index and checks each
/// track (the rest), and however it is paged.
#[tokio::test]
async fn filters_by_tags_of_every_type() {
    let (_temp, _db, app) = tagged_library().await;
    let cases: [(&str, Keep); 15] = [
        ("tag=30", |i| i % 3 == 0),
        ("tag=30,31", |i| i % 3 == 0 || i % 5 == 0),
        ("tag=30,31&tagMatch=all", |i| i % 15 == 0),
        ("tag=30,32", |i| i % 6 == 0),
        ("tag=31,30,32", |i| (i % 3 == 0 || i % 5 == 0) && i % 2 == 0),
        ("tag=33", |i| i == 15),
        ("tag=33,30,31&tagMatch=all", |i| i == 15),
        ("tag=33,32", |i| i == 15 || i % 2 == 0),
        ("tag=33,32&tagMatch=all", |_| false),
        ("tag=33,31", |i| i == 15),
        ("tag=33,34", |_| false),
        ("tag=30&albumId=4", |i| i % 3 == 0 && i % 10 == 3),
        ("tag=30&artistId=9", |i| i % 12 == 0),
        ("tag=30,50", |_| false),
        ("tag=30,nope", |_| false),
    ];
    for (query, keep) in cases {
        let expected: Vec<Value> =
            (1..=300).filter(|&i| keep(i)).map(|i| json!(i.to_string())).collect();
        let base = format!("/api/v1/libraries/1/tracks?{query}");
        for limit in [1, 7, 1000] {
            assert_eq!(
                page_through(&app, &base, limit, "id").await,
                json!(expected),
                "{query}, limit {limit}"
            );
        }
        let mut reversed = expected.clone();
        reversed.reverse();
        let newest =
            page_through(&app, &format!("{base}&sort=dateAdded&order=desc"), 7, "id").await;
        assert_eq!(newest, json!(reversed), "{query}, newest first");
        let (_, page) = json(&app, &base).await;
        assert_eq!(page["total"], expected.len(), "{query}");
    }
    let (_, other) = json(&app, "/api/v1/libraries/2/tracks?tag=50").await;
    assert_eq!(other["items"][0]["id"], "400");
    assert_eq!(json(&app, "/api/v1/libraries/2/tracks?tag=30").await.1["total"], 0);
    let (status, _) = json(&app, "/api/v1/libraries/1/tracks?tag=30&tagMatch=most").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
