//! Planner statistics wait for a library of real size (`design/database.md` §4).

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use jewelcase_scanner::*;

use super::{make, record};
use crate::db::store::SqliteStore;
use crate::db::{Database, FIRST_SWEEP, STATISTICS_FROM_TRACKS, libraries, optimize};

/// How many rows of planner statistics describe the tracks after running [`optimize`], as the
/// writer does every few minutes.
fn track_statistics(store: &SqliteStore) -> i64 {
    store
        .db()
        .write_blocking(|tx| {
            optimize(tx);
            Ok(())
        })
        .unwrap();
    stored_statistics(store.db())
}

/// How many rows of planner statistics `db` holds for the tracks.
fn stored_statistics(db: &Database) -> i64 {
    db.read_blocking(|conn| {
        let gathered: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name = 'sqlite_stat1')",
            [],
            |r| r.get(0),
        )?;
        if !gathered {
            return Ok(0);
        }
        conn.query_row("SELECT COUNT(*) FROM sqlite_stat1 WHERE tbl = 'tracks'", [], |r| r.get(0))
    })
    .unwrap()
}

/// The writer gathers no statistics for a library of one track, which would describe its tables
/// as a row each once they have grown, and gathers them for one of [`STATISTICS_FROM_TRACKS`].
#[test]
fn statistics_wait_for_a_library_of_real_size() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    store.apply(1, Batch { upserts: vec![record(&root, 0)], ..Batch::default() }).unwrap();
    assert_eq!(track_statistics(&store), 0, "one track");

    let tracks = usize::try_from(STATISTICS_FROM_TRACKS).unwrap();
    for start in (1..tracks).step_by(250) {
        let upserts = (start..tracks.min(start + 250)).map(|i| record(&root, i)).collect();
        store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    }
    assert!(track_statistics(&store) > 0, "{tracks} tracks");
}

/// Opening the database gathers the statistics it lacks only for a library of real size too.
#[test]
fn opening_gathers_statistics_only_for_a_library_of_real_size() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let path = tmp.path().join("state/jewelcase.db");
    let store = make(&root);
    store.apply(1, Batch { upserts: vec![record(&root, 0)], ..Batch::default() }).unwrap();
    drop(store);
    assert_eq!(stored_statistics(&Database::open(&path).unwrap()), 0, "one track");

    let store = make(&root);
    let tracks = usize::try_from(STATISTICS_FROM_TRACKS).unwrap();
    for start in (1..tracks).step_by(250) {
        let upserts = (start..tracks.min(start + 250)).map(|i| record(&root, i)).collect();
        store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    }
    drop(store);
    assert!(stored_statistics(&Database::open(&path).unwrap()) > 0, "{tracks} tracks");
}

/// Opening gathers statistics within SQLite's analysis limit, so a first open of a large
/// database reads only part of each index. Past the limit, a limited analysis skips the rest of
/// each run of equal keys, so it finds fewer rows per key than a full one counts.
#[test]
fn opening_gathers_statistics_within_the_analysis_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("jewelcase.db");
    drop(Database::open(&path).unwrap());
    // Filled while the database is closed, so nothing analyzes it before it opens again.
    let mut conn = rusqlite::Connection::open(&path).unwrap();
    let tx = conn.transaction().unwrap();
    libraries::create(&tx, Some(1), "Music", &[], &[]).unwrap();
    tx.execute_batch(
        "UPDATE libraries SET track_count = 1000; \
         CREATE TABLE large (n INTEGER); CREATE INDEX large_n ON large (n); \
         WITH RECURSIVE n(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM n WHERE n < 10000) \
         INSERT INTO large SELECT n % 10 FROM n;",
    )
    .unwrap();
    tx.commit().unwrap();
    drop(conn);

    let stat: String = Database::open(&path)
        .unwrap()
        .read_blocking(|conn| {
            conn.query_row("SELECT stat FROM sqlite_stat1 WHERE idx = 'large_n'", [], |r| r.get(0))
        })
        .unwrap();
    assert_ne!(stat, "10000 1000", "counted in full");
}

/// How the writer would run the `tracks_added` trigger's update of the artists a track on album
/// `?1` counts toward.
fn owned_count_plan(db: &Database) -> String {
    let explain = "EXPLAIN QUERY PLAN UPDATE artists SET owned_track_count = owned_track_count + 1 \
                   WHERE id IN (SELECT artist_id FROM album_artists WHERE album_id = ?1)";
    db.write_blocking(move |tx| {
        let mut plan = tx.prepare(explain)?;
        let steps = plan.query_map([0], |r| r.get::<_, String>(3))?;
        steps.collect::<rusqlite::Result<Vec<_>>>().map(|steps| steps.join("; "))
    })
    .unwrap()
}

/// How many rows `db`'s statistics give `artists`, or 0 while they do not describe it.
fn described_artists(db: &Database) -> i64 {
    let sql =
        "SELECT ifnull(max(CAST(stat AS INTEGER)), 0) FROM sqlite_stat1 WHERE tbl = 'artists'";
    db.read_blocking(move |conn| conn.query_row(sql, [], |r| r.get(0))).unwrap()
}

/// Statistics gathered while 1,200 tracks have 12 artists describe `artists` as a few rows, which
/// sends each new track's trigger to read every artist. Once 200 more artists arrive, the writer
/// gathers them again in its periodic optimize, with nothing else asked of it, and the trigger
/// looks artists up.
#[test]
fn a_table_grown_tenfold_is_described_again_while_the_server_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    let upserts = |range: std::ops::Range<usize>, newcomers: bool| {
        let records = range.map(|i| {
            let mut record = record(&root, i);
            if newcomers {
                record.tags.artists = vec![format!("Newcomer {i}")];
                record.tags.album_artists = record.tags.artists.clone();
            }
            record
        });
        Batch { upserts: records.collect(), ..Batch::default() }
    };
    for start in (0..1_200).step_by(250) {
        store.apply(1, upserts(start..(start + 250).min(1_200), false)).unwrap();
    }
    drop(store);
    // Opening it again gathers statistics for the 1,200 tracks, and the writer gathers them again
    // every half second, after the new artists have arrived.
    let path = tmp.path().join("state/jewelcase.db");
    let every = Duration::from_millis(500);
    let db = Arc::new(Database::open_timed(&path, FIRST_SWEEP, every).unwrap());
    let store = SqliteStore::new(db.clone()).unwrap();
    store.apply(1, upserts(1_200..1_400, true)).unwrap();

    // Read from the readers, so the writer is given no job to run its optimize after.
    for _ in 0..1_500 {
        if described_artists(&db) > 200 {
            let plan = owned_count_plan(&db);
            assert!(!plan.contains("SCAN artists"), "{plan}");
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("the statistics still describe {} artists after 30 s", described_artists(&db));
}
