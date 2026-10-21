//! The whole path, with the real plugins: a hook event reaches the plugin's `handle`, and what
//! it answers is recorded. Neither is granted the network it requires, so neither reaches its
//! service; both are enabled as only the database allows.

use std::time::Duration;

use jewelcase_plugins::Permission::{LibraryRead, ListeningActivity, Played, TracksChanged};
use rusqlite::OptionalExtension;
use serde_json::json;

use super::{changes, play};
use crate::db::Database;
use crate::db::testing::{User, access};
use crate::plugins::PluginError;
use crate::plugins::testing::{connect, database, enable_ungranted, grant, plugins};

/// Waits for the one delivery under way to finish, and returns its run's summary and the
/// failures and user its cursor records. A delivery records its cursor last, after the run's
/// summary, so both are in place once the cursor is. The deadline only bounds a hang: plugins
/// compile and run slowly on a busy machine.
async fn delivered(db: &Database) -> (Option<String>, i64, i64) {
    let recorded = async {
        loop {
            let found = db
                .read(|conn| {
                    conn.query_row(
                        "SELECT l.last_run_summary, c.failures, c.user_id FROM plugin_cursors c \
                         JOIN plugin_libraries l USING (plugin_id, library_id)",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()
                })
                .await
                .unwrap();
            match found {
                Some(found) => return found,
                None => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(30), recorded).await.expect("delivered within 30 s")
}

/// The real lyrics plugin hears of a change in the feed as `tracks-changed`, finds it can't
/// look the track up without the network, and the failure is recorded to retry.
#[tokio::test]
async fn delivers_changes_to_a_real_plugin() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/lrclib-lyrics/target/lrclib-lyrics.wasm"
    );
    let id = plugins
        .install(std::fs::read(file).expect("run plugins/lrclib-lyrics/build.sh first"), None)
        .await
        .unwrap();
    grant(&plugins, &id, &[], &[LibraryRead, TracksChanged]).await;
    enable_ungranted(&db, "lrclib-lyrics").await;
    changes(&db, "(1, 'track', 11, 'upsert', 0)").await;

    plugins.dispatch().await;
    let refused = "the plugin reported an error: a permission it needs was not granted";
    assert_eq!(delivered(&db).await, (Some(refused.to_owned()), 1, 0), "one failure, to retry");
}

/// The real ListenBrainz plugin: checking user 7's token and delivering their play both reach
/// the plugin and are refused when it tries to send, and the play is kept for user 7 to send
/// again.
#[tokio::test]
async fn delivers_plays_to_a_real_scrobbler() {
    const REFUSED: &str = "the plugin reported an error: Network access was not granted";
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    let file =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/listenbrainz/target/listenbrainz.wasm");
    let id = plugins
        .install(std::fs::read(file).expect("run plugins/listenbrainz/build.sh first"), None)
        .await
        .unwrap();
    grant(&plugins, &id, &[ListeningActivity, Played], &[LibraryRead]).await;
    enable_ungranted(&db, "listenbrainz").await;
    db.write(|tx| {
        User::new(7, "user").insert(tx)?;
        access(tx, 7, 1)
    })
    .await
    .unwrap();

    let token = json!({ "token": "a-token" });
    let refused = plugins.save_personal(id.clone(), 7, false, token.as_object().unwrap().clone());
    match refused.await {
        Err(PluginError::SettingsInvalid(why)) => assert_eq!(why, REFUSED),
        other => panic!("{other:?}"),
    }

    // Connected as if it had been accepted, then a play long enough to count.
    connect(&db, "listenbrainz", 7, token, 0).await;
    play(&db, "a", 7, 11, 1_790_942_400_000, "finished").await;
    db.write(|tx| tx.execute_batch("UPDATE plays SET listen_time_ms = 180000")).await.unwrap();

    plugins.dispatch().await;
    let kept = (Some(REFUSED.to_owned()), 1, 7);
    assert_eq!(delivered(&db).await, kept, "kept for user 7 to send again");
}
