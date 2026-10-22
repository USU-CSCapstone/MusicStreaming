//! Benchmarks of the API on a large generated library, ignored unless asked for.

use std::path::Path;
use std::time::{Duration, Instant};

use axum::http::StatusCode;

use super::testing::{app, json, send};
use crate::db::store::SqliteStore;
use crate::db::testing::library;
use crate::db::testing::scale::{self, summary};

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
/// `BUDGET_TRACKS` sets the library size (default 500,000).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark"]
async fn benchmark_list_pages() {
    let (_temp, db, app) = app("");
    let filling = db.clone();
    tokio::task::spawn_blocking(move || {
        filling.write_blocking(|tx| library(tx, 1, "Music", "/music")).unwrap();
        let store = SqliteStore::new(filling).unwrap();
        scale::fill(&store, scale::records(Path::new("/music"), scale::size()));
    })
    .await
    .unwrap();
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
        let budget = Some(Duration::from_millis(100));
        println!("{uri}\n  {}", summary(&mut times, budget));
    }
}
