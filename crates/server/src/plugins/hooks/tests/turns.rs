//! A delivery that waits for the plugin's turn is found due again once it has it, for the same
//! user. The hook loop polls for deliveries, and nothing starts once the hooks are stopped.

use std::sync::Arc;
use std::time::Duration;

use jewelcase_plugins::Permission::{
    LibraryRead, ListeningActivity, Played, Schedule, TracksChanged,
};
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use tokio::sync::oneshot;

use super::super::{Delivering, POLL};
use super::failing::changed;
use super::{changes, connected, play, setup};
use crate::db::Database;
use crate::plugins::Plugins;
use crate::plugins::testing::{connect, database, enable, grant, install, manifest, plugins};

/// Waits until `count` runs hold or wait for the plugin's turn in library 1. The deadline only
/// bounds a hang.
async fn queued(plugins: &Plugins, count: usize) {
    let queued = async {
        while plugins.turns.waiting("plugin", 1) < count {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(Duration::from_secs(30), queued).await.expect("queued within 30 s");
}

/// Waits for the plugin's turn in library 1, once every run ahead of it ends. The deadline only
/// bounds a hang.
async fn ended(plugins: &Plugins) {
    let turn = tokio::time::timeout(Duration::from_secs(30), plugins.turns.take("plugin", 1));
    drop(turn.await.expect("the runs end within 30 s"));
}

/// User `user`'s position, and whether it is failing, in `played` in library 1, if recorded.
async fn cursor(db: &Database, user: i64) -> Option<(i64, bool)> {
    let sql = "SELECT position, failing_since IS NOT NULL FROM plugin_cursors \
               WHERE hook = 'played' AND library_id = 1 AND user_id = ?1";
    db.read(move |conn| {
        let found = conn.query_row(sql, params![user], |row| Ok((row.get(0)?, row.get(1)?)));
        rusqlite::OptionalExtension::optional(found)
    })
    .await
    .unwrap()
}

/// Users 7 and 8 both connected the plugin and ended plays in library 1. While their deliveries
/// wait for the plugin's turn, user 7 loses the library: theirs records nothing, and user 8's is
/// delivered. The plugin has no file, so user 8's run is recorded as one that could not load.
#[tokio::test]
async fn a_delivery_for_a_user_who_lost_the_library_meanwhile_runs_nothing() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Played]).await;
    connect(&db, "plugin", 8, json!({}), 100).await;
    play(&db, "a", 7, 11, 200, "finished").await;
    play(&db, "b", 8, 12, 210, "finished").await;
    let turn = plugins.turns.take("plugin", 1).await;

    plugins.dispatch().await;
    queued(&plugins, 3).await;
    let revoke = "DELETE FROM library_access WHERE user_id = 7";
    db.write(move |tx| tx.execute_batch(revoke)).await.unwrap();
    drop(turn);
    ended(&plugins).await;

    assert_eq!(cursor(&db, 7).await, None, "nothing recorded for user 7");
    assert_eq!(cursor(&db, 8).await, Some((0, true)), "user 8's run could not load");
}

/// A delivery found due from a position its plugin no longer has, as after the plugin is
/// uninstalled and installed again while the delivery waits for its turn, runs nothing and
/// records nothing, so the new install's position starts over.
#[tokio::test]
async fn a_delivery_from_a_position_since_forgotten_runs_nothing() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    let asks = manifest("plugin", &[LibraryRead, TracksChanged]);
    install(&db, asks.clone()).await;
    grant(&plugins, "plugin", &[], &[LibraryRead, TracksChanged]).await;
    enable(&plugins, "plugin", true).await;
    changes(&db, "(1, 'track', 11, 'upsert', 0), (1, 'track', 12, 'upsert', 0)").await;
    let past_first = "INSERT INTO plugin_cursors (plugin_id, library_id, hook, position) \
                      SELECT 'plugin', 1, 'tracksChanged', min(seq) FROM library_changes";
    db.write(move |tx| tx.execute_batch(past_first)).await.unwrap();

    // In the queue for the turn, in order: the uninstall, the reinstall, then the delivery.
    let turn = plugins.turns.take("plugin", 1).await;
    let uninstall = tokio::spawn({
        let plugins = plugins.clone();
        async move { plugins.uninstall("plugin".into()).await }
    });
    queued(&plugins, 2).await;
    let ((taken, took), (go, going)) = (oneshot::channel(), oneshot::channel::<()>());
    let reinstalling = tokio::spawn({
        let plugins = plugins.clone();
        async move {
            let turn = plugins.turns.take("plugin", 1).await;
            taken.send(()).unwrap();
            going.await.unwrap();
            drop(turn);
        }
    });
    queued(&plugins, 3).await;
    plugins.dispatch().await;
    queued(&plugins, 4).await;

    drop(turn);
    uninstall.await.unwrap().unwrap();
    took.await.unwrap();
    install(&db, asks).await;
    enable(&plugins, "plugin", true).await;
    go.send(()).unwrap();
    reinstalling.await.unwrap();
    ended(&plugins).await;

    let sql = "SELECT count(*) FROM plugin_cursors";
    let cursors: i64 =
        db.read(move |conn| conn.query_row(sql, [], |row| row.get(0))).await.unwrap();
    assert_eq!(cursors, 0, "nothing recorded from the old position");
}

/// The summary of the last run in library 1, if there was one.
async fn last_run(db: &Database) -> Option<String> {
    let sql = "SELECT last_run_summary FROM plugin_libraries WHERE library_id = 1";
    let read = db.read(move |conn| conn.query_row(sql, [], |row| row.get(0)).optional());
    read.await.unwrap().flatten()
}

/// Once the hooks are stopped, as the server stops, a dispatch starts nothing, even with a
/// delivery due.
#[tokio::test]
async fn a_dispatch_once_the_hooks_are_stopped_starts_nothing() {
    let (_temp, _db, plugins) = setup().await;
    grant(&plugins, "plugin", &[Schedule], &[]).await;
    plugins.stop_hooks();
    plugins.dispatch().await;
    assert!(plugins.delivering.lock().unwrap().is_empty(), "a delivery started");
}

/// A dispatch with nothing due starts no plugin runtime.
#[tokio::test]
async fn a_dispatch_with_nothing_due_starts_no_runtime() {
    let (_temp, _db, plugins) = setup().await;
    plugins.dispatch().await;
    assert!(plugins.runtime.get().is_none(), "it started the plugin runtime");
}

/// A delivery that waits for a hook slot as the hooks are stopped runs nothing once it has one,
/// and records nothing. The deadlines only bound a hang.
#[tokio::test]
async fn a_delivery_waiting_as_the_hooks_stop_runs_nothing() {
    let (temp, db) = changed().await;
    let plugins = plugins(&temp, &db);
    let all = u32::try_from(plugins.hook_slots.available_permits()).unwrap();
    let slots = plugins.hook_slots.acquire_many(all).await.unwrap();
    plugins.dispatch().await;
    queued(&plugins, 1).await;

    plugins.stop_hooks();
    drop(slots);
    let key = ("plugin".to_owned(), 1, TracksChanged, None);
    let ended = async {
        while Delivering::claim(&plugins, key.clone()).is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(30), ended).await.expect("it ends");
    assert_eq!(last_run(&db).await, None, "it ran");
    let sql = "SELECT count(*) FROM plugin_cursors";
    let recorded: i64 =
        db.read(move |conn| conn.query_row(sql, [], |row| row.get(0))).await.unwrap();
    assert_eq!(recorded, 0, "it recorded an outcome");
}

/// The loop, started with nothing enabled, delivers within a poll of a plugin being enabled: this
/// one is scheduled, so it is due at once, and has no file, so its run fails as soon as it
/// starts, and is recorded. The clock is paused and moved on a poll at a time.
#[tokio::test(start_paused = true)]
async fn the_hook_loop_delivers_within_a_poll_of_a_plugin_being_enabled() {
    let (temp, db) = database().await;
    let mut scheduled = manifest("plugin", &[Schedule]);
    scheduled["permissions"][0]["everyMinutes"] = json!(60);
    install(&db, scheduled).await;
    let plugins = plugins(&temp, &db);
    grant(&plugins, "plugin", &[Schedule], &[]).await;
    plugins.start_hooks();
    // The loop holds a strong reference through a dispatch: wait for the first to end, so the
    // delivery must come from a later poll.
    while Arc::strong_count(&plugins) == 1 {
        tokio::task::yield_now().await;
    }
    while Arc::strong_count(&plugins) > 1 {
        last_run(&db).await;
    }
    enable(&plugins, "plugin", true).await;
    for _ in 0..1000 {
        tokio::time::advance(POLL).await;
        // Time moves a poll at a time, so give the delivery's blocking work real time.
        tokio::time::sleep(Duration::from_millis(1)).await;
        if last_run(&db).await.is_some() {
            return;
        }
    }
    panic!("nothing was delivered");
}
