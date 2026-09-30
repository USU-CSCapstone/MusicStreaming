//! Benchmarks of the API at the verification hardware's scale, ignored unless asked for.

use std::path::Path;

use axum::http::StatusCode;

use super::testing::{app, json, send};
use crate::db::libraries;

/// What a browse page costs at the Pi's library size (`requirements/performance.md` §1, §3):
/// 100,000 tracks on 10,000 albums by 5,000 artists, each track with one or two artists and a
/// genre, through the router so serializing counts too. Run with
/// `cargo test --release -p jewelcase-server list_pages -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn list_pages() {
    let (_temp, db, app) = app("");
    db.write(|tx| {
        libraries::create(tx, Some(1), "Music", &[Path::new("/music")], &[])?;
        tx.execute_batch(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000)
             INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at, updated_at)
             SELECT i, 1, 'Artist ' || i, 'artist ' || i, CAST(printf('artist %06d', (i * 7919) % 5000) AS BLOB), i, 0 FROM n;
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 50)
             INSERT INTO tags (id, library_id, type, name, name_key, sort_key)
             SELECT 100000 + i, 1, 'genre', 'Genre ' || i, 'genre ' || i, CAST('genre ' || i AS BLOB) FROM n;
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10000)
             INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key, artist_sort_key,
                                 track_count, available_track_count, disc_count, added_at, updated_at)
             SELECT 200000 + i, 1, 'Album ' || i, 'album ' || i, 'k' || i,
                    CAST(printf('album %06d', (i * 7919) % 10000) AS BLOB), x'', 10, 10, 1, i, 0 FROM n;
             INSERT INTO album_artists (library_id, album_id, position, artist_id)
             SELECT 1, id, 0, 1 + (id % 5000) FROM albums;
             INSERT INTO album_tags (library_id, album_id, tag_id) SELECT 1, id, 100001 + (id % 50) FROM albums;
             INSERT INTO album_discs (library_id, album_id, disc_number, track_count, track_total)
             SELECT 1, id, 1, 10, 10 FROM albums;
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100000)
             INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size, file_mtime, title,
                                 sort_key, artist_sort_key, album_sort_key, disc_number, track_number,
                                 codec, container, lossless, sample_rate_hz, channels, duration_us,
                                 added_at, updated_at)
             SELECT 1000000 + i, 1, 200001 + (i % 10000), (SELECT id FROM library_roots), 'p' || i, 1, 0,
                    'Track ' || i, CAST(printf('track %08d', (i * 7919) % 100000) AS BLOB), x'', x'', 1, 1 + i / 10000,
                    'flac', 'flac', 1, 44100, 2, 1, i, 0 FROM n;
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
    for (list, limit) in [
        ("tracks", 100),
        ("tracks", 1000),
        ("albums", 100),
        ("albums", 1000),
        ("artists", 100),
    ] {
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
        println!(
            "{list:>8} limit {limit:>4}: median {:>9.2?}  p95 {:>9.2?}",
            times[30], times[57]
        );
    }
}
