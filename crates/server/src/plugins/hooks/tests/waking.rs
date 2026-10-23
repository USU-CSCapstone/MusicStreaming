//! The hook loop sleeps while no plugin is enabled, starting nothing, wakes when an admin enables
//! one, and ends when the plugins are dropped.

use std::sync::Arc;
use std::time::Duration;

use jewelcase_plugins::Permission::{Schedule, TracksChanged};
use rusqlite::OptionalExtension;
use serde_json::json;
use tokio::sync::Notify;
use tokio::time::{Instant, timeout};

use super::super::{Delivering, POLL, idle};
use super::outages::changed;
use super::setup;
use crate::api::testing::eventually;
use crate::db::Database;
use crate::plugins::testing::{database, enable, grant, install, manifest, plugins};

/// The summary of the last run in library 1, if there was one.
async fn last_run(db: &Database) -> Option<String> {
    let sql = "SELECT last_run_summary FROM plugin_libraries WHERE library_id = 1";
    let read = db.read(move |conn| conn.query_row(sql, [], |row| row.get(0)).optional());
    read.await.unwrap().flatten()
}

#[tokio::test]
async fn dispatch_says_whether_any_plugin_is_enabled() {
    let (_temp, _db, plugins) = setup().await;
    // Set up enabled but granted nothing, so nothing is due, and nothing is started.
    assert!(plugins.dispatch().await);
    enable(&plugins, "plugin", false).await;
    assert!(!plugins.dispatch().await);
    assert!(plugins.runtime.get().is_none(), "it started the plugin runtime");
}

/// Once the hooks are stopped, as the server stops, a dispatch starts nothing, even with a
/// delivery due.
#[tokio::test]
async fn a_dispatch_once_the_hooks_are_stopped_starts_nothing() {
    let (_temp, _db, plugins) = setup().await;
    grant(&plugins, "plugin", &[Schedule], &[]).await;
    plugins.stop_hooks();
    assert!(!plugins.dispatch().await);
    assert!(plugins.delivering.lock().unwrap().is_empty(), "a delivery started");
}

/// A delivery that waits for a hook slot as the hooks are stopped runs nothing once it has one,
/// and records nothing. The deadlines only bound a hang.
#[tokio::test]
async fn a_delivery_waiting_as_the_hooks_stop_runs_nothing() {
    let (temp, db) = changed().await;
    let plugins = plugins(&temp, &db);
    let all = u32::try_from(plugins.hook_slots.available_permits()).unwrap();
    let slots = plugins.hook_slots.acquire_many(all).await.unwrap();
    assert!(plugins.dispatch().await);
    let holding = async {
        while plugins.turns.waiting("plugin", 1) == 0 {
            tokio::task::yield_now().await;
        }
    };
    timeout(Duration::from_secs(30), holding).await.expect("it takes its turn");

    plugins.stop_hooks();
    drop(slots);
    let key = ("plugin".to_owned(), 1, TracksChanged, None);
    let ended = async {
        while Delivering::claim(&plugins, key.clone()).is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    timeout(Duration::from_secs(30), ended).await.expect("it ends");
    assert_eq!(last_run(&db).await, None, "it ran");
    let sql = "SELECT count(*) FROM plugin_cursors";
    let recorded: i64 =
        db.read(move |conn| conn.query_row(sql, [], |row| row.get(0))).await.unwrap();
    assert_eq!(recorded, 0, "it recorded an outcome");
}

/// A failure to read what is due is no sign that nothing is enabled, so polling goes on.
#[tokio::test]
async fn dispatch_that_cannot_read_what_is_due_polls_again() {
    let (_temp, db, plugins) = setup().await;
    db.write(|tx| tx.execute_batch("DROP TABLE plugin_cursors")).await.unwrap();
    assert!(plugins.dispatch().await);
}

/// With nothing enabled the loop waits for a wake, however long that takes; with a plugin
/// enabled it polls. The clock is paused, so nothing here depends on how fast the machine is.
#[tokio::test(start_paused = true)]
async fn the_hook_loop_sleeps_until_woken_while_nothing_is_enabled() {
    let wake = Notify::new();
    assert!(timeout(POLL * 100, idle(false, &wake)).await.is_err(), "it woke on its own");
    wake.notify_one();
    assert!(timeout(Duration::ZERO, idle(false, &wake)).await.is_ok(), "the wake was missed");
    let started = Instant::now();
    assert!(timeout(POLL * 2, idle(true, &wake)).await.is_ok(), "it did not poll");
    assert_eq!(started.elapsed(), POLL);
}

/// Enabling a plugin leaves a wake for the loop, whether or not it is asleep yet; disabling one
/// leaves none.
#[tokio::test]
async fn enabling_a_plugin_wakes_the_hook_loop() {
    let (temp, db) = database().await;
    install(&db, manifest("plugin", &[])).await;
    let plugins = plugins(&temp, &db);
    let woken = async || timeout(Duration::ZERO, plugins.wake_hooks.notified()).await.is_ok();
    assert!(!woken().await);
    enable(&plugins, "plugin", true).await;
    assert!(woken().await, "enabling it left no wake");
    enable(&plugins, "plugin", false).await;
    assert!(!woken().await, "disabling it left a wake");
}

/// The loop, started with nothing enabled, delivers once a plugin is: this one is scheduled, so
/// it is due at once, and has no file, so its run fails as soon as it starts, and is recorded.
#[tokio::test]
async fn the_hook_loop_delivers_once_a_plugin_is_enabled() {
    let (temp, db) = database().await;
    let mut scheduled = manifest("plugin", &[Schedule]);
    scheduled["permissions"][0]["everyMinutes"] = json!(60);
    install(&db, scheduled).await;
    let plugins = plugins(&temp, &db);
    grant(&plugins, "plugin", &[Schedule], &[]).await;
    plugins.start_hooks();
    enable(&plugins, "plugin", true).await;
    eventually("a delivery", async || last_run(&db).await).await;
}

/// The loop holds the wake's other handle, and drops it as it ends.
#[tokio::test]
async fn dropping_the_plugins_ends_the_hook_loop() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    let wake = plugins.wake_hooks.clone();
    plugins.start_hooks();
    // Long enough for the loop to find nothing enabled, and sleep.
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(plugins);
    eventually("the loop ending", async || (Arc::strong_count(&wake) == 1).then_some(())).await;
}
