//! The analysis queue against a real database: its order, and that a pick sorts nothing.

use std::sync::Arc;

use rusqlite::Connection;

use super::{SqliteStore, UNANALYZED, next_unanalyzed};
use crate::db::Database;
use crate::db::testing::{Album, Track, library};

/// A store whose library 1 holds album 1 with `tracks`, each `(id, analyzer version, added
/// at, missing)`, and the directory its database lives in.
fn store(tracks: &[(i64, i64, i64, bool)]) -> (tempfile::TempDir, SqliteStore) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&dir.path().join("jewelcase.db")).unwrap());
    let tracks = tracks.to_vec();
    let root = dir.path().join("music");
    db.write_blocking(move |tx| {
        library(tx, 1, "Music", root)?;
        Album::new(1, 1).insert(tx)?;
        for (id, version, added_at, missing) in tracks {
            let missing_since = missing.then_some(1);
            Track { added_at, missing_since, ..Track::new(id, 1, 1) }.insert(tx)?;
            tx.execute("UPDATE tracks SET analyzer_version = ?2 WHERE id = ?1", [id, version])?;
        }
        Ok(())
    })
    .unwrap();
    (dir, SqliteStore::new(db).unwrap())
}

/// The steps of the plan for the queue's query at analyzer version `version`.
fn plan(conn: &Connection, version: i64) -> rusqlite::Result<Vec<String>> {
    conn.prepare(&format!("EXPLAIN QUERY PLAN {UNANALYZED}"))?
        .query_map([1, version, 1], |row| row.get(3))?
        .collect()
}

#[test]
fn never_analyzed_tracks_come_first_and_newest_first() {
    let (_dir, store) = store(&[
        (1, 0, 10, false),
        (2, 0, 30, false),
        (3, 1, 40, false),
        (4, 1, 20, false),
        (5, 2, 50, false),
        (6, 0, 60, true),
    ]);
    let queue = next_unanalyzed(&store, 1, 2, 10).unwrap();
    let ids: Vec<_> = queue.iter().map(|(track, _)| track.id).collect();
    assert_eq!(ids, [2, 1, 3, 4], "not the current version's, nor the missing one");
}

/// Sorting the library instead costs 300–550 ms a pick at 500k tracks (`design/database.md`
/// §8).
#[test]
fn a_pick_reads_the_head_of_one_index_range() {
    let tracks: Vec<_> = (1..=200).map(|id| (id, 1, id, false)).collect();
    let (_dir, store) = store(&tracks);
    let plans = store
        .db()
        .write_blocking(|tx| {
            // Statistics of a library where every track is analyzed, the idle poll's case.
            tx.execute_batch("ANALYZE")?;
            Ok([plan(tx, 1)?, plan(tx, 2)?])
        })
        .unwrap();
    for steps in plans {
        assert_eq!(
            steps,
            ["SEARCH tracks USING INDEX tracks_unanalyzed (library_id=? AND analyzer_version<?)"]
        );
    }
}
