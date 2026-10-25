//! Planner statistics wait for a library of real size (`design/database.md` §4).

use jewelcase_scanner::*;

use super::{make, record};
use crate::db::store::SqliteStore;
use crate::db::{Database, STATISTICS_FROM_TRACKS, optimize};

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
