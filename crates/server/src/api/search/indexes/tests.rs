use rusqlite::Transaction;

use super::*;
use crate::api::testing::catalog;
use crate::db::testing::{Album, Artist, Track};

/// An empty index read at names `version`, just finished after taking `took`.
fn built(version: i64, took: Duration) -> Built {
    Built { index: Arc::default(), version, ready: Instant::now(), took }
}

#[test]
fn rebuilds_are_spaced_by_how_long_the_last_took() {
    // A slow build just finished: the library has changed, but it is too soon.
    assert!(!built(1, Duration::from_secs(60)).due(2));
    // A quick one is rebuilt at once.
    assert!(built(1, Duration::ZERO).due(2));
    // Nothing has changed.
    assert!(!built(1, Duration::ZERO).due(1));
}

/// A write to the catalog.
type Write = fn(&Transaction) -> rusqlite::Result<()>;

/// Library 1's names version.
async fn version(db: &Database) -> i64 {
    db.read(|conn| library_number(conn, 1, "names_version")).await.unwrap()
}

#[tokio::test]
async fn only_what_search_indexes_moves_the_names_version() {
    let (_temp, db, _app) = catalog::app().await;
    let changes: [(&str, Write, bool); 16] = [
        (
            "analysis",
            |tx| tx.execute_batch("UPDATE tracks SET loudness_lufs = -7 WHERE id = 1001"),
            false,
        ),
        (
            "a placeholder",
            |tx| tx.execute_batch("UPDATE images SET placeholder = x'00' WHERE id = 40"),
            false,
        ),
        (
            "a feed row",
            |tx| {
                tx.execute_batch(
                    "INSERT INTO library_changes (library_id, entity_type, entity_id, op, changed_at)
                     VALUES (1, 'album', 101, 'upsert', 0)",
                )
            },
            false,
        ),
        (
            "a track's title rewritten",
            |tx| tx.execute_batch("UPDATE tracks SET title = title"),
            false,
        ),
        (
            "an album's title rewritten",
            |tx| tx.execute_batch("UPDATE albums SET title = title"),
            false,
        ),
        (
            "an artist's name rewritten",
            |tx| tx.execute_batch("UPDATE artists SET name = name"),
            false,
        ),
        (
            "a track renamed",
            |tx| tx.execute_batch("UPDATE tracks SET title = 'Here' WHERE id = 1001"),
            true,
        ),
        (
            "an album renamed",
            |tx| tx.execute_batch("UPDATE albums SET title = 'Here' WHERE id = 101"),
            true,
        ),
        (
            "the unknown album named",
            |tx| tx.execute_batch("UPDATE albums SET title = 'Here' WHERE id = 104"),
            true,
        ),
        (
            "an artist renamed",
            |tx| tx.execute_batch("UPDATE artists SET name = 'Here' WHERE id = 10"),
            true,
        ),
        ("a track added", |tx| Track::new(1007, 1, 101).insert(tx), true),
        ("a track deleted", |tx| tx.execute_batch("DELETE FROM tracks WHERE id = 1007"), true),
        ("an album added", |tx| Album::new(105, 1).insert(tx), true),
        ("an album deleted", |tx| tx.execute_batch("DELETE FROM albums WHERE id = 105"), true),
        ("an artist added", |tx| Artist::new(14, 1, "New").insert(tx), true),
        ("an artist deleted", |tx| tx.execute_batch("DELETE FROM artists WHERE id = 14"), true),
    ];
    for (change, write, moves) in changes {
        let before = version(&db).await;
        db.write(write).await.unwrap();
        assert_eq!(version(&db).await != before, moves, "{change}");
    }
}

/// Library 1's index before `change`, and after it, once any rebuild it makes due is done.
async fn around(change: &'static str) -> (Arc<Index>, Arc<Index>) {
    let (_temp, db, _app) = catalog::app().await;
    let indexes = Indexes::default();
    let before = indexes.index(&db, 1).await.unwrap();
    db.write(move |tx| tx.execute_batch(change)).await.unwrap();
    let slot = indexes.libraries.lock().unwrap()[&1].clone();
    let took = slot.built.read().unwrap().as_ref().unwrap().took;
    tokio::time::sleep(took * REBUILD_SPACING).await;
    // This starts a rebuild if the change calls for one, and the lock waits for it.
    indexes.index(&db, 1).await.unwrap();
    drop(slot.building.lock().await);
    (before, indexes.index(&db, 1).await.unwrap())
}

#[tokio::test]
async fn a_write_that_names_nothing_keeps_the_index() {
    let (before, after) = around(
        "UPDATE tracks SET loudness_lufs = -7, title = title WHERE id = 1001;
         INSERT INTO library_changes (library_id, entity_type, entity_id, op, changed_at)
         VALUES (1, 'track', 1001, 'upsert', 0);",
    )
    .await;
    assert!(Arc::ptr_eq(&before, &after));
}

#[tokio::test]
async fn a_rename_rebuilds_the_index() {
    let (before, after) = around("UPDATE tracks SET title = 'Here' WHERE id = 1001").await;
    assert!(!Arc::ptr_eq(&before, &after));
    let found = after.search("here", &[jewelcase_core::Kind::Track], 1);
    assert_eq!(found[0].ids, [1001]);
}

#[tokio::test]
async fn a_deleted_librarys_index_goes_and_none_is_made_for_it() {
    let (_temp, db, _app) = catalog::app().await;
    let indexes = Indexes::default();
    indexes.index(&db, 2).await.unwrap();
    db.write(|tx| tx.execute_batch("DELETE FROM libraries WHERE id = 2")).await.unwrap();
    indexes.index(&db, 1).await.unwrap();
    assert_eq!(indexes.libraries.lock().unwrap().keys().collect::<Vec<_>>(), [&1]);
    let index = indexes.index(&db, 2).await.unwrap();
    assert!(index.search("elsewhere", &jewelcase_core::Kind::ALL, 1).is_empty());
    assert_eq!(indexes.libraries.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn every_library_is_indexed_at_startup() {
    let (_temp, db, _app) = catalog::app().await;
    let indexes = Arc::new(Indexes::default());
    indexes.build_all(&db);
    crate::api::testing::eventually("both libraries are indexed", async || {
        let libraries = indexes.libraries.lock().unwrap();
        let built =
            |id| libraries.get(&id).is_some_and(|slot| slot.built.read().unwrap().is_some());
        (built(1) && built(2)).then_some(())
    })
    .await;
}
