//! A failure on the server's side is retried without counting toward disabling the plugin, but a
//! plugin that cannot be loaded fails like any other. A delivery lets the next one start once it
//! ends, even by a panic.

use std::sync::Arc;
use std::time::Duration;

use jewelcase_plugins::Permission::TracksChanged;
use rusqlite::OptionalExtension;

use super::super::{BACKOFF_MS, Delivering, Ran};
use super::failing::{REFUSED, changed, enabled, hooks};
use super::{HOUR, ran};
use crate::api::testing::eventually;
use crate::db::Database;
use crate::plugins::Plugins;
use crate::plugins::testing::plugins;

/// SQLite's message for a full disk: a failure on the server's side, not the plugin's.
const DISK_FULL: &str = "the database or disk is full";

/// Dispatches once, and answers whether the first delivery is recorded as failing, and whether
/// it waits to be retried. The deadline only bounds a hang.
async fn dispatched(plugins: &Arc<Plugins>, db: &Database) -> (bool, bool) {
    plugins.dispatch().await;
    let sql = "SELECT failing_since IS NOT NULL, retry_at IS NOT NULL FROM plugin_cursors";
    eventually("the delivery is recorded", async || {
        let row = |row: &rusqlite::Row| Ok((row.get(0)?, row.get(1)?));
        db.read(move |conn| conn.query_row(sql, [], row).optional()).await.unwrap()
    })
    .await
}

/// A plugin that cannot be loaded, here one installed without its file, fails and is retried
/// like any other, so it is disabled and reported once that has lasted long enough, rather than
/// retried forever.
#[tokio::test]
async fn a_plugin_that_cannot_be_loaded_is_failing() {
    let (temp, db) = changed().await;
    assert_eq!(dispatched(&plugins(&temp, &db), &db).await, (true, true));
}

/// A run the server fails to start, here for settings it cannot read, is retried without
/// counting. SQLite takes a number too large for serde as JSON.
#[tokio::test]
async fn a_run_the_server_fails_is_retried_without_counting() {
    let (temp, db) = changed().await;
    let unreadable = "INSERT INTO plugin_settings (plugin_id, library_id, settings) \
                      VALUES ('plugin', 0, '{\"limit\": 1e400}')";
    db.write(move |tx| tx.execute(unreadable, [])).await.unwrap();
    assert_eq!(dispatched(&plugins(&temp, &db), &db).await, (false, true));
}

#[tokio::test]
async fn a_failure_on_the_servers_side_backs_off_but_never_counts() {
    let (_temp, db) = changed().await;
    let tried = [TracksChanged];
    assert_eq!(hooks(ran(&db, 0, Ran::Unrun, DISK_FULL).await), tried);
    assert!(ran(&db, BACKOFF_MS - 1, Ran::Unrun, DISK_FULL).await.is_empty(), "waiting");
    let mut now = BACKOFF_MS;
    for attempt in 1..=75 {
        assert_eq!(hooks(ran(&db, now, Ran::Unrun, DISK_FULL).await), tried, "{attempt}");
        now += HOUR;
    }
    assert!(enabled(&db).await, "the server's failures alone never disable it");

    // Nor do they count as the plugin failing: its own failure only now begins.
    assert_eq!(hooks(ran(&db, now, Ran::Failed, REFUSED).await), tried);
    assert_eq!(hooks(ran(&db, now + 72 * HOUR, Ran::Unrun, DISK_FULL).await), tried);
    assert!(enabled(&db).await, "the server's failure disables nothing");
    assert_eq!(hooks(ran(&db, now + 73 * HOUR, Ran::Failed, REFUSED).await), tried);
    assert!(!enabled(&db).await, "nor does it end the failing");
}

/// A delivery that panics still lets its key go, so the next one can start.
#[tokio::test]
async fn a_delivery_that_panics_lets_the_next_one_start() {
    let (temp, db) = changed().await;
    let plugins = plugins(&temp, &db);
    let key = ("plugin".to_owned(), 1, TracksChanged, None);
    let delivering = Delivering::claim(&plugins, key.clone()).unwrap();
    assert!(Delivering::claim(&plugins, key.clone()).is_none(), "one at a time");
    let panicked = tokio::spawn(async move {
        let _held = delivering;
        panic!("a delivery panicked");
    });
    assert!(panicked.await.unwrap_err().is_panic());
    assert!(Delivering::claim(&plugins, key).is_some());
}

/// A delivery that ends lets its key go, so the next one can start. The deadline only bounds a
/// hang.
#[tokio::test]
async fn a_delivery_lets_the_next_one_start_once_it_ends() {
    let (temp, db) = changed().await;
    let plugins = plugins(&temp, &db);
    dispatched(&plugins, &db).await;
    let key = ("plugin".to_owned(), 1, TracksChanged, None);
    let released = async {
        while Delivering::claim(&plugins, key.clone()).is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), released).await.unwrap();
}

/// A delivery whose outcome cannot be recorded, here for a disk the database finds full, lets
/// the plugin's turn and its hook slot go at once, but keeps its claim for [`BACKOFF_MS`], so a
/// run whose effects may have happened is not repeated at every poll. The deadline only bounds a
/// hang.
#[tokio::test]
async fn a_delivery_that_cannot_be_recorded_backs_off() {
    let (temp, db) = changed().await;
    let refuse = format!(
        "CREATE TRIGGER full_on_insert BEFORE INSERT ON plugin_cursors \
         BEGIN SELECT RAISE(ABORT, '{DISK_FULL}'); END; \
         CREATE TRIGGER full_on_update BEFORE UPDATE ON plugin_cursors \
         BEGIN SELECT RAISE(ABORT, '{DISK_FULL}'); END;"
    );
    db.write(move |tx| tx.execute_batch(&refuse)).await.unwrap();
    let plugins = plugins(&temp, &db);
    let slots = plugins.hook_slots.available_permits();
    plugins.dispatch().await;
    // Its report is recorded as the library's last run before its outcome is.
    let sql = "SELECT last_run_at IS NOT NULL FROM plugin_libraries WHERE library_id = 1";
    let reported = async {
        while !db
            .read(move |conn| conn.query_row(sql, [], |row| row.get::<_, bool>(0)))
            .await
            .unwrap()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(30), reported).await.unwrap();

    let turn = tokio::time::timeout(Duration::from_secs(10), plugins.turns.take("plugin", 1));
    drop(turn.await.expect("its turn is let go"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(plugins.hook_slots.available_permits(), slots, "and its slot");
    let key = ("plugin".to_owned(), 1, TracksChanged, None);
    assert!(Delivering::claim(&plugins, key).is_none(), "but not its claim");
}
