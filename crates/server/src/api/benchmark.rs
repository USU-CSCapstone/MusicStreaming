//! Benchmarks of the API at the verification hardware's scale, ignored unless asked for.

use axum::http::StatusCode;

use super::testing::{app, json, send};
use crate::db::testing::{Album, Artist, Track, library, scattered_key};

/// What a browse page costs at the Pi's library size (`requirements/performance.md` §1, §3):
/// 100,000 tracks on 10,000 albums by 5,000 artists, each track with one or two artists and a
/// genre, through the router so serializing counts too. Run with
/// `cargo test --release -p jewelcase-server benchmark_list_pages -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark"]
async fn benchmark_list_pages() {
    let (_temp, db, app) = app("");
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        for i in 1..=5000 {
            let artist = Artist::new(i, 1, &format!("Artist {i}"));
            Artist { sort_key: scattered_key("artist", i), added_at: i, ..artist }.insert(tx)?;
        }
        for i in 1..=10_000 {
            let album = Album {
                title: Some(format!("Album {i}")),
                sort_key: scattered_key("album", i),
                track_count: 10,
                available_track_count: 10,
                disc_count: 1,
                added_at: i,
                ..Album::new(200_000 + i, 1)
            };
            album.insert(tx)?;
        }
        for i in 1..=100_000 {
            let track = Track {
                path: format!("p{i}"),
                sort_key: scattered_key("track", i),
                disc_number: 1,
                track_number: Some(1 + i / 10_000),
                added_at: i,
                ..Track::new(1_000_000 + i, 1, 200_001 + i % 10_000)
            };
            track.insert(tx)?;
        }
        tx.execute_batch(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 50)
             INSERT INTO tags (id, library_id, type, name, name_key, sort_key)
             SELECT 100000 + i, 1, 'genre', 'Genre ' || i, 'genre ' || i, CAST('genre ' || i AS BLOB) FROM n;
             INSERT INTO album_artists (library_id, album_id, position, artist_id)
             SELECT 1, id, 0, 1 + (id % 5000) FROM albums;
             INSERT INTO album_tags (library_id, album_id, tag_id) SELECT 1, id, 100001 + (id % 50) FROM albums;
             INSERT INTO album_discs (library_id, album_id, disc_number, track_count, track_total)
             SELECT 1, id, 1, 10, 10 FROM albums;
             INSERT INTO track_artists (library_id, track_id, position, artist_id)
             SELECT 1, id, 0, 1 + (id % 5000) FROM tracks;
             INSERT INTO track_artists (library_id, track_id, position, artist_id)
             SELECT 1, id, 1, 1 + ((id + 1) % 5000) FROM tracks WHERE id % 3 = 0;
             INSERT INTO track_tags (library_id, track_id, tag_id, tag_name)
             SELECT 1, id, 100001 + (id % 50), 'x' FROM tracks;
             UPDATE libraries SET track_count = 100000, album_count = 10000, artist_count = 5000;
             ANALYZE;",
        )
    })
    .await
    .unwrap();
    for (list, limit) in
        [("tracks", 100), ("tracks", 1000), ("albums", 100), ("albums", 1000), ("artists", 100)]
    {
        let uri = format!("/api/v1/libraries/1/{list}?limit={limit}");
        let (_, first) = json(&app, &uri).await;
        let deep = format!("{uri}&cursor={}", first["nextCursor"].as_str().unwrap());
        let mut times = Vec::new();
        for i in 0..60 {
            let started = std::time::Instant::now();
            let (status, _, _) =
                send(app.clone(), "GET", if i % 2 == 0 { &uri } else { &deep }).await;
            assert_eq!(status, StatusCode::OK);
            times.push(started.elapsed());
        }
        times.sort();
        println!("{list:>8} limit {limit:>4}: median {:>9.2?}  p95 {:>9.2?}", times[30], times[57]);
    }
}
