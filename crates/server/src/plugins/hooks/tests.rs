use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use jewelcase_plugins::Permission;
use rusqlite::Connection;

use super::super::Plugins;
use super::super::testing::database;
use super::due::{all, batch, event};
use super::{BACKOFF_MS, Due, Hook, MAX_FAILURES, record};
use crate::db::Database;

/// Plugin `p` enabled in library 1 with the hook and read access granted there.
async fn subscribed(db: &Database) {
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO plugin_library_grants VALUES ('p', 1, 'tracksChanged'), ('p', 1, 'libraryRead');
             INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES ('p', 1, 1);",
        )
    })
    .await
    .unwrap();
}

async fn changes(db: &Database, rows: &'static str) {
    db.write(move |tx| {
        tx.execute_batch(&format!(
            "INSERT INTO library_changes (library_id, entity_type, entity_id, op, at) VALUES {rows}"
        ))
    })
    .await
    .unwrap();
}

async fn run<T: Send + 'static>(
    db: &Database,
    f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
) -> T {
    db.read(f).await.unwrap()
}

/// What is due now for `hook`: each plugin, library, and position.
async fn due_for(db: &Database, hook: Hook, now: i64) -> Vec<(String, i64, i64)> {
    let due = run(db, move |conn| all(conn, now)).await;
    due.into_iter().filter(|d| d.hook == hook).map(|d| (d.plugin, d.library, d.position)).collect()
}

async fn due_pairs(db: &Database, now: i64) -> Vec<(String, i64, i64)> {
    due_for(db, Hook::TracksChanged, now).await
}

fn due(hook: Hook, failures: i64) -> Due {
    Due { hook, plugin: "p".into(), library: 1, position: 0, failures }
}

#[tokio::test]
async fn a_pair_is_due_while_enabled_with_both_grants_and_changes_waiting() {
    let (_temp, db) = database().await;
    subscribed(&db).await;
    assert!(due_pairs(&db, 0).await.is_empty(), "nothing has changed");

    changes(&db, "(1, 'track', 11, 'upsert', 0), (2, 'track', 21, 'upsert', 0)").await;
    assert_eq!(due_pairs(&db, 0).await, [("p".to_owned(), 1, 0)], "only its own library");

    for (unset, restore) in [
        (
            "DELETE FROM plugin_library_grants WHERE permission = 'libraryRead'",
            "INSERT INTO plugin_library_grants VALUES ('p', 1, 'libraryRead')",
        ),
        (
            "DELETE FROM plugin_library_grants WHERE permission = 'tracksChanged'",
            "INSERT INTO plugin_library_grants VALUES ('p', 1, 'tracksChanged')",
        ),
        ("UPDATE plugin_libraries SET enabled = 0", "UPDATE plugin_libraries SET enabled = 1"),
    ] {
        db.write(move |tx| tx.execute_batch(unset)).await.unwrap();
        assert!(due_pairs(&db, 0).await.is_empty(), "{unset}");
        db.write(move |tx| tx.execute_batch(restore)).await.unwrap();
    }

    db.write(|tx| {
        tx.execute_batch("INSERT INTO plugin_cursors (plugin_id, library_id, hook, retry_at) VALUES ('p', 1, 'tracksChanged', 1000)")
    })
    .await
    .unwrap();
    assert!(due_pairs(&db, 999).await.is_empty(), "waiting to retry");
    assert_eq!(due_pairs(&db, 1000).await.len(), 1);

    db.write(|tx| {
        tx.execute_batch(
            "UPDATE plugin_cursors SET position = (SELECT max(seq) FROM library_changes)",
        )
    })
    .await
    .unwrap();
    assert!(due_pairs(&db, 1000).await.is_empty(), "delivered up to the end");
}

#[tokio::test]
async fn a_batch_names_the_tracks_changed_and_removed() {
    let (_temp, db) = database().await;
    changes(&db, "(1, 'track', 11, 'upsert', 0), (1, 'album', 101, 'upsert', 0), (1, 'track', 12, 'delete', 0), (2, 'track', 21, 'upsert', 0)").await;
    let (next, tracks) = run(&db, |conn| batch(conn, 1, 0)).await;
    assert_eq!((tracks.changed, tracks.removed), (vec![11], vec![12]));
    let (after, rest) = run(&db, move |conn| batch(conn, 1, next)).await;
    assert_eq!(after, next, "nothing further in library 1");
    assert!(rest.changed.is_empty() && rest.removed.is_empty());
}

#[tokio::test]
async fn a_batch_is_at_most_a_hundred_changes() {
    let (_temp, db) = database().await;
    db.write(|tx| {
        tx.execute_batch(
            "WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 250)
             INSERT INTO library_changes (library_id, entity_type, entity_id, op, at)
             SELECT 1, 'track', 1000 + i, 'upsert', 0 FROM n",
        )
    })
    .await
    .unwrap();
    let (next, first) = run(&db, |conn| batch(conn, 1, 0)).await;
    let (_, second) = run(&db, move |conn| batch(conn, 1, next)).await;
    assert_eq!((first.changed.len(), second.changed.len()), (100, 100));
    assert_eq!((first.changed[0], second.changed[0]), (1001, 1101));
}

async fn cursor(db: &Database) -> (i64, i64, Option<i64>) {
    run(db, |conn| {
        conn.query_row("SELECT position, failures, retry_at FROM plugin_cursors", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
    })
    .await
}

#[tokio::test]
async fn failures_wait_longer_each_time_then_disable_the_plugin_there() {
    let (_temp, db) = database().await;
    subscribed(&db).await;
    let fail = |failures: i64| {
        let db = db.clone();
        async move {
            db.write(move |tx| {
                record(tx, &due(Hook::TracksChanged, failures), 99, false, "LRCLIB answered 503", 0)
            })
            .await
            .unwrap();
        }
    };
    fail(0).await;
    assert_eq!(cursor(&db).await, (0, 1, Some(BACKOFF_MS)), "the position stays");
    fail(1).await;
    assert_eq!(cursor(&db).await, (0, 2, Some(2 * BACKOFF_MS)));
    for failures in 2..MAX_FAILURES {
        fail(failures).await;
    }
    let (enabled, reason): (bool, String) = run(&db, |conn| {
        conn.query_row("SELECT enabled, disabled_reason FROM plugin_libraries", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
    })
    .await;
    assert!(!enabled);
    assert_eq!(reason, "Stopped after failing 5 times in a row: LRCLIB answered 503");

    db.write(|tx| record(tx, &due(Hook::TracksChanged, 5), 99, true, "", 0)).await.unwrap();
    assert_eq!(cursor(&db).await, (99, 0, None), "a success moves on and clears the failures");
}

#[tokio::test]
async fn a_finished_scan_is_delivered_once_with_its_counts_added_up() {
    let (_temp, db) = database().await;
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO plugin_library_grants VALUES ('p', 1, 'scanFinished'), ('p', 1, 'libraryRead');
             INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES ('p', 1, 1);
             INSERT INTO scans (id, library_id, trigger, state, scopes, added, updated, created_at, finished_at)
             VALUES (1, 1, 'watch', 'completed', '[]', 2, 1, 0, 100),
                    (2, 1, 'watch', 'completed', '[]', 3, 0, 0, 200),
                    (3, 1, 'watch', 'running', '[]', 9, 9, 0, NULL),
                    (4, 2, 'watch', 'completed', '[]', 9, 9, 0, 150);",
        )
    })
    .await
    .unwrap();
    assert!(due_for(&db, Hook::ScanFinished, 100).await.is_empty(), "the first finished only now");
    assert_eq!(due_for(&db, Hook::ScanFinished, 201).await, [("p".to_owned(), 1, 0)]);

    let (next, delivered) = run(&db, |conn| event(conn, &due(Hook::ScanFinished, 0), 201)).await;
    let Some(jewelcase_plugins::Event::ScanFinished(totals)) = delivered else {
        panic!("no scan event")
    };
    assert_eq!(
        (next, totals.added, totals.updated),
        (200, 5, 1),
        "both scans, not the running one"
    );
    db.write(move |tx| record(tx, &due(Hook::ScanFinished, 0), next, true, "", 0)).await.unwrap();
    assert!(due_for(&db, Hook::ScanFinished, 1000).await.is_empty(), "each is delivered once");
}

#[tokio::test]
async fn a_schedule_is_due_when_its_interval_has_passed() {
    let (_temp, db) = database().await;
    db.write(|tx| {
        tx.execute_batch(
            r#"UPDATE plugins SET manifest = '{"permissions":[{"permission":"schedule","everyMinutes":60}]}';
               INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES ('p', 1, 1);"#,
        )
    })
    .await
    .unwrap();
    assert!(due_for(&db, Hook::Schedule, 0).await.is_empty(), "not granted");
    db.write(|tx| tx.execute_batch("INSERT INTO plugin_grants VALUES ('p', 'schedule')"))
        .await
        .unwrap();
    assert_eq!(due_for(&db, Hook::Schedule, 0).await.len(), 1, "at once, the first time");

    let hour = 60 * 60_000;
    db.write(move |tx| record(tx, &due(Hook::Schedule, 0), 1000, true, "", 1000)).await.unwrap();
    assert!(due_for(&db, Hook::Schedule, 1000 + hour - 1).await.is_empty());
    assert_eq!(due_for(&db, Hook::Schedule, 1000 + hour).await.len(), 1);
}

/// The whole path with the real lyrics plugin: a change in the feed reaches its `handle` as
/// `tracks-changed`. It is granted everything but the network it requires, so it reports that
/// rather than reaching lrclib.net, and the failure is recorded to retry.
#[tokio::test]
#[ignore = "needs plugins/lrclib-lyrics/build.sh"]
async fn delivers_changes_to_a_real_plugin() {
    let (temp, db) = database().await;
    db.write(|tx| tx.execute_batch("DELETE FROM plugins")).await.unwrap();
    let plugins = Arc::new(Plugins::new(db.clone(), temp.path().join("plugins"), HashMap::new()));
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/lrclib-lyrics/target/lrclib-lyrics.wasm"
    );
    let id = plugins.install(std::fs::read(file).unwrap(), None).await.unwrap();
    let granted = vec![Permission::LibraryRead, Permission::TracksChanged];
    plugins.set_grants(id.clone(), Vec::new(), vec![(1, granted)]).await.unwrap();
    // The network is required, so enabling is refused; enable it as the database allows.
    db.write(|tx| tx.execute_batch("INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES ('lrclib-lyrics', 1, 1)")).await.unwrap();
    changes(&db, "(1, 'track', 11, 'upsert', 0)").await;

    plugins.dispatch().await;
    for _ in 0..200 {
        let summary: Option<String> = run(&db, |conn| {
            conn.query_row("SELECT last_run_summary FROM plugin_libraries", [], |row| row.get(0))
        })
        .await;
        if let Some(summary) = summary {
            assert_eq!(
                summary,
                "the plugin reported an error: needs permission to use the network"
            );
            assert_eq!(cursor(&db).await.1, 1, "one failure, to retry");
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the change was not delivered");
}
