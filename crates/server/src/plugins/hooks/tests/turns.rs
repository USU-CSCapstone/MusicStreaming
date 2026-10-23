//! A delivery that waits for the plugin's turn is found due again once it has it, for the same
//! user.

use std::time::Duration;

use jewelcase_plugins::Permission::{LibraryRead, ListeningActivity, Played, TracksChanged};
use rusqlite::params;
use serde_json::json;
use tokio::sync::oneshot;

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

/// User `user`'s position, failures, and outages in `played` in library 1, if recorded.
async fn cursor(db: &Database, user: i64) -> Option<(i64, i64, i64)> {
    let sql = "SELECT position, failures, outages FROM plugin_cursors \
               WHERE hook = 'played' AND library_id = 1 AND user_id = ?1";
    db.read(move |conn| {
        let found =
            conn.query_row(sql, params![user], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)));
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
    assert_eq!(cursor(&db, 8).await, Some((0, 0, 1)), "user 8's run could not load");
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
