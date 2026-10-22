//! The hook loop sleeps while no plugin is enabled, starting nothing, wakes when an admin enables
//! one, and ends when the plugins are dropped.

use std::sync::Arc;
use std::time::Duration;

use jewelcase_plugins::Permission::Schedule;
use rusqlite::OptionalExtension;
use serde_json::json;

use super::super::POLL;
use super::setup;
use crate::api::testing::eventually;
use crate::db::Database;
use crate::plugins::testing::{
    database, enable, enable_ungranted, grant, install, manifest, plugins,
};

/// The summary of the last run in library 1, if there was one.
async fn last_run(db: &Database) -> Option<String> {
    let sql = "SELECT last_run_summary FROM plugin_libraries WHERE library_id = 1";
    let read = db.read(move |conn| conn.query_row(sql, [], |row| row.get(0)).optional());
    read.await.unwrap().flatten()
}

/// Waits `real` wall-clock time without moving a test's paused clock, for the database and the
/// plugin runtime, which work in real time.
async fn wait(real: Duration) {
    tokio::task::spawn_blocking(move || std::thread::sleep(real)).await.unwrap();
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

/// A failure to read what is due is no sign that nothing is enabled, so polling goes on.
#[tokio::test]
async fn dispatch_that_cannot_read_what_is_due_polls_again() {
    let (_temp, db, plugins) = setup().await;
    db.write(|tx| tx.execute_batch("DROP TABLE plugin_cursors")).await.unwrap();
    assert!(plugins.dispatch().await);
}

/// The plugin is scheduled, so it is due at once, and has no file, so its run fails as soon as
/// it starts, and is recorded. It is first enabled straight in the database, which wakes nothing,
/// so only the wake from an admin enabling it can start the run, however long the loop sleeps
/// before that.
#[tokio::test(start_paused = true)]
async fn the_hook_loop_sleeps_until_an_admin_enables_a_plugin() {
    let (temp, db) = database().await;
    let mut scheduled = manifest("plugin", &[Schedule]);
    scheduled["permissions"][0]["everyMinutes"] = json!(60);
    install(&db, scheduled).await;
    let plugins = plugins(&temp, &db);
    grant(&plugins, "plugin", &[Schedule], &[]).await;
    plugins.start_hooks();
    wait(Duration::from_millis(100)).await;
    enable_ungranted(&db, "plugin").await;
    tokio::time::sleep(POLL * 3).await;
    wait(Duration::from_millis(200)).await;
    assert!(plugins.runtime.get().is_none(), "it delivered without being woken");
    assert_eq!(last_run(&db).await, None);
    enable(&plugins, "plugin", true).await;
    for _ in 0..1500 {
        if last_run(&db).await.is_some() {
            return;
        }
        wait(Duration::from_millis(20)).await;
    }
    panic!("nothing was delivered within 30 s");
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
