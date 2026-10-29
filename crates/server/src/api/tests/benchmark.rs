//! Benchmarks of the API on a large generated library, ignored unless asked for.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::http::StatusCode;

use super::super::AnyId;
use super::super::page::{Order, Request, Sort, Unknown};
use super::super::tag_filter::{LEAD_SHARE, TagFilter, TagMatch, Tags};
use super::super::testing::{app, json, send};
use super::super::tracks;
use crate::db::Database;
use crate::db::store::SqliteStore;
use crate::db::testing::library;
use crate::db::testing::scale::{self, summary};

/// Library 1's artist with the most tracks, the unknown artist of its untagged files, and an
/// artist of one album, as the API spells their IDs.
const ARTISTS: &str = "SELECT id FROM (SELECT id FROM artists ORDER BY track_count DESC LIMIT 1) \
     UNION ALL SELECT id FROM artists WHERE name IS NULL \
     UNION ALL SELECT id FROM (SELECT id FROM artists WHERE album_count = 1 ORDER BY id LIMIT 1)";

/// Library 1's tags to filter by, fewest tracks first: every mood ([`scale::MOODS`]), and its
/// largest, middle and smallest genres, each with its ID, name and track count.
const TAGS: &str = "SELECT id, name, track_count FROM ( \
     SELECT id, name, track_count FROM tags WHERE type = 'mood' \
     UNION SELECT * FROM (SELECT id, name, track_count FROM tags WHERE type = 'genre' \
                          ORDER BY track_count DESC LIMIT 1) \
     UNION SELECT * FROM (SELECT id, name, track_count FROM tags WHERE type = 'genre' \
                          ORDER BY track_count LIMIT 1 \
                          OFFSET (SELECT count(*) / 2 FROM tags WHERE type = 'genre')) \
     UNION SELECT * FROM (SELECT id, name, track_count FROM tags WHERE type = 'genre' \
                          ORDER BY track_count LIMIT 1)) ORDER BY track_count, id";

/// Tracks by name, as `listTracks` pages them by default.
const BY_NAME: Sort =
    Sort { label: "tracks.name", columns: &["t.sort_key", "t.id"], unknown: Unknown::Empty };

/// The router over library 1 filled with the generated library ([`scale::records`]) of
/// [`scale::size`] tracks.
async fn filled() -> (tempfile::TempDir, Arc<Database>, Router) {
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

/// Times 60 requests for `uri` through the router, alternately its first page and, where it has
/// one, the page its first page's cursor leads to, and prints them.
async fn time_pages(app: &Router, uri: &str) {
    let (_, first) = json(app, uri).await;
    let deep = match first["nextCursor"].as_str() {
        Some(cursor) => format!("{uri}&cursor={cursor}"),
        None => uri.to_owned(),
    };
    let mut times = Vec::new();
    for i in 0..60 {
        let started = Instant::now();
        let (status, _, _) = send(app.clone(), "GET", if i % 2 == 0 { uri } else { &deep }).await;
        assert_eq!(status, StatusCode::OK);
        times.push(started.elapsed());
    }
    println!("{uri}\n  {}", summary(&mut times));
}

/// What a browse page costs on the generated library, through the router so serializing counts
/// too (`requirements/performance.md` §3): each list unfiltered, and tracks and albums of the
/// artist with the most tracks, of the unknown artist, and of an artist with one album, each a
/// first and a deep page; a list that fits one page has no deep page, so its first page is timed
/// throughout. The database goes in the temporary folder, so run it with that on an SSD, not a
/// tmpfs:
/// `TMPDIR=<folder> cargo test --release -p jewelcase-server benchmark_list_pages -- --ignored --nocapture`.
/// `BENCH_TRACKS` sets the library size (default 500,000). It prints each page's timings and fails
/// only on a wrong answer, never on a time.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark"]
async fn benchmark_list_pages() {
    let (_temp, db, app) = filled().await;
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
        time_pages(&app, &uri).await;
    }
}

/// Reads tracks by name carrying tag `tag`, a group leading when it holds one track in `share` at
/// most ([`Tags::plan_sharing`]): 30 first pages of 100 and 30 deep ones, alternately, and how
/// long each took the database.
fn time_reads(db: &Database, tag: i64, share: i64) -> Vec<Duration> {
    db.read_blocking(move |conn| {
        let filter = TagFilter { ids: vec![AnyId(Some(tag))], tag_match: TagMatch::Any };
        let rows = super::super::page::library_number(conn, 1, "track_count")?;
        let read = |cursor: Option<String>| {
            let tags = Tags::read(conn, 1, &tracks::TAGGED, &filter)?;
            let plan = tags.plan_sharing(&tracks::TAGGED, "t", None, rows, true, share);
            let mut source = tracks::TRACKS.source(conn, 1, plan.lead, plan.check)?;
            source.total = plan.total.unwrap();
            let request = Request::new(&BY_NAME, Order::Asc, cursor, Some(100)).unwrap();
            request.read(conn, &source, tracks::list_summary)
        };
        let deep = read(None)?.next_cursor;
        let mut times = Vec::new();
        for i in 0..60 {
            let started = Instant::now();
            read(if i % 2 == 0 { None } else { deep.clone() })?;
            times.push(started.elapsed());
        }
        Ok(times)
    })
    .unwrap()
}

/// What a tag filter costs on the generated library, for tags from one track in a million to a
/// third of the library (`design/database.md` §4): the page the API reads, then the same pages
/// read each way the filter can, starting from the tag's tracks and walking the name index, to
/// weigh where one should give way to the other (`LEAD_SHARE`); and a search for every tagged
/// title, narrowed to the tag, which builds the tag's set of tracks each time. Run as
/// [`benchmark_list_pages`] is, with `benchmark_tag_filters`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark"]
async fn benchmark_tag_filters() {
    let (_temp, db, app) = filled().await;
    let tags: Vec<(i64, String, i64)> = db
        .read(|conn| {
            conn.prepare(TAGS)?.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect()
        })
        .await
        .unwrap();
    let rows =
        db.read(|conn| super::super::page::library_number(conn, 1, "track_count")).await.unwrap();
    println!("{rows} tracks; a tag leads at {} tracks or fewer", rows / LEAD_SHARE);

    let lists = "/api/v1/libraries/1";
    time_pages(&app, &format!("{lists}/search?q=track")).await;
    for (tag, name, count) in tags {
        println!("\n{name}: {count} tracks");
        time_pages(&app, &format!("{lists}/tracks?tag={tag}&limit=100")).await;
        for (way, share) in [("lead", 0), ("walk", i64::MAX)] {
            let db = db.clone();
            let mut times =
                tokio::task::spawn_blocking(move || time_reads(&db, tag, share)).await.unwrap();
            println!("  {way}  {}", summary(&mut times));
        }
        time_pages(&app, &format!("{lists}/albums?tag={tag}&limit=100")).await;
        time_pages(&app, &format!("{lists}/search?q=track&tag={tag}")).await;
    }
}
