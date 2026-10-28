//! Benchmarks of the API on a large generated library, ignored unless asked for.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::http::StatusCode;
use serde_json::json;

use super::super::testing::{TOKEN, app, call, json, send};
use crate::db::Database;
use crate::db::store::SqliteStore;
use crate::db::testing::library;
use crate::db::testing::scale::{self, summary};

/// The app with the generated library ([`scale::records`]) as library 1, at `BENCH_TRACKS` tracks.
async fn generated_library() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app("");
    let filling = db.clone();
    tokio::task::spawn_blocking(move || {
        filling.write_blocking(|tx| library(tx, 1, "Music", "/music")).unwrap();
        let store = SqliteStore::new(filling).unwrap();
        scale::fill(&store, scale::records(Path::new("/music"), scale::size()));
    })
    .await
    .unwrap();
    (temp, db, app)
}

/// Library 1's artist with the most tracks, the unknown artist of its untagged files, and an
/// artist of one album, as the API spells their IDs.
const ARTISTS: &str = "SELECT id FROM (SELECT id FROM artists ORDER BY track_count DESC LIMIT 1) \
     UNION ALL SELECT id FROM artists WHERE name IS NULL \
     UNION ALL SELECT id FROM (SELECT id FROM artists WHERE album_count = 1 ORDER BY id LIMIT 1)";

/// What a browse page costs on the generated library ([`scale::records`]), through the router
/// so serializing counts too (`requirements/performance.md` §3): each list unfiltered, and
/// tracks and albums of the artist with the most tracks, of the unknown artist, and of an artist
/// with one album, each a first and a deep page; a list that fits one page has no deep page, so
/// its first page is timed throughout. The database goes in the temporary folder, so run it with
/// that on an SSD, not a tmpfs:
/// `TMPDIR=<folder> cargo test --release -p jewelcase-server benchmark_list_pages -- --ignored --nocapture`.
/// `BENCH_TRACKS` sets the library size (default 500,000). It prints each page's timings and fails
/// only on a wrong answer, never on a time.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark"]
async fn benchmark_list_pages() {
    let (_temp, db, app) = generated_library().await;
    let artists: Vec<i64> = db
        .read(|conn| conn.prepare(ARTISTS)?.query_map([], |r| r.get(0))?.collect())
        .await
        .unwrap();

    let lists = "/api/v1/libraries/1";
    let unfiltered = [
        "tracks?limit=100",
        "tracks?limit=1000",
        "albums?limit=100",
        "albums?limit=1000",
        "artists?limit=100",
    ];
    let mut pages = Vec::from(unfiltered.map(|page| format!("{lists}/{page}")));
    for artist in artists {
        pages.push(format!("{lists}/tracks?artistId={artist}&limit=100"));
        pages.push(format!("{lists}/albums?artistId={artist}&limit=100"));
        pages.push(format!("{lists}/artists/{artist}"));
    }
    for uri in pages {
        let (_, first) = json(&app, &uri).await;
        let deep = match first["nextCursor"].as_str() {
            Some(cursor) => format!("{uri}&cursor={cursor}"),
            None => uri.clone(),
        };
        let mut times = Vec::new();
        for i in 0..60 {
            let started = Instant::now();
            let (status, _, _) =
                send(app.clone(), "GET", if i % 2 == 0 { &uri } else { &deep }).await;
            assert_eq!(status, StatusCode::OK);
            times.push(started.elapsed());
        }
        println!("{uri}\n  {}", summary(&mut times));
    }
}

/// What starting a queue from a list query costs on the generated library, through the router:
/// the whole library by name and by artist descending, and the artist with the most tracks, each
/// started at its first track; the server reads the whole list whichever track starts it. A
/// search start is not timed: it runs the index's ranking, which `core`'s full-scale search
/// benchmark covers. Run it as [`benchmark_list_pages`] is:
/// `TMPDIR=<folder> cargo test --release -p jewelcase-server benchmark_queue_start -- --ignored --nocapture`.
/// It prints each start's time and fails only on a wrong answer, never on a time.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark"]
async fn benchmark_queue_start() {
    let (_temp, db, app) = generated_library().await;
    let tracks = scale::size();
    let artist: i64 = db.read(|conn| conn.query_row(ARTISTS, [], |r| r.get(0))).await.unwrap();
    let first = |sort: &str, order: &str, artist: Option<i64>| {
        let artist = artist.map_or(String::new(), |artist| format!("&artistId={artist}"));
        format!("/api/v1/libraries/1/tracks?sort={sort}&order={order}&limit=1{artist}")
    };
    let starts = [
        ("library by name".to_owned(), json!({ "type": "tracks" }), first("name", "asc", None)),
        (
            "library by artist, descending".to_owned(),
            json!({ "type": "tracks", "sort": "artist", "order": "desc" }),
            first("artist", "desc", None),
        ),
        (
            "the artist with the most tracks".to_owned(),
            json!({ "type": "tracks", "artistId": artist.to_string() }),
            first("name", "asc", Some(artist)),
        ),
    ];
    for (n, (name, query, first_track)) in starts.into_iter().enumerate() {
        let (_, page) = json(&app, &first_track).await;
        let track = page["items"][0]["id"].as_str().unwrap().to_owned();
        let body = json!({
            "queueId": format!("5f0c6a52-3b7e-4d43-9a8e-0d1c2b3a{n:04x}"),
            "context": { "type": "library" }, "startTrackId": track,
            "source": { "key": "0b1b1b1b-3b7e-4d43-9a8e-0d1c2b3a4f5e", "libraryId": "1", "query": query },
        });
        let started = Instant::now();
        let (status, _) = call(&app, TOKEN, "POST", "/api/v1/me/queue", Some(body)).await;
        let took = started.elapsed();
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (_, snapshot) = json(&app, "/api/v1/me/queue").await;
        let queued = snapshot["state"]["sources"][0]["tracks"].as_array().unwrap().len();
        if n < 2 {
            assert_eq!(queued, tracks, "{name}");
        }
        println!("{name}: {queued} of {tracks} tracks in {took:.1?}");
    }
}
