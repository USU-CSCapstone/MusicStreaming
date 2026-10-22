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
use crate::plugins::testing::{connect, database, enable_ungranted, grant, plugins};
use crate::plugins::{PluginError, Plugins};

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

/// A recorded run's report without the compute it spent, which varies, once checked to end
/// with it: `summary (refused …; ` or `summary (`.
fn uncounted(report: Option<String>) -> String {
    let report = report.expect("a run was recorded");
    let (head, compute) = report.rsplit_once("about ").expect("its compute");
    assert!(
        compute.ends_with(" ms of compute)") || compute.ends_with(" s of compute)"),
        "{report}"
    );
    head.to_owned()
}

/// What the real lyrics plugin answers a change without the network.
const LOOKUP_REFUSED: &str = "the plugin reported an error: a permission it needs was not granted";

/// The real lyrics plugin installed, granted `tracksChanged` and enabled in library 1 without
/// the network, with `change` in the feed for it.
async fn lyrics(plugins: &Plugins, db: &Database, change: &'static str) {
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/lrclib-lyrics/target/lrclib-lyrics.wasm"
    );
    let id = plugins
        .install(std::fs::read(file).expect("run plugins/lrclib-lyrics/build.sh first"), None)
        .await
        .unwrap();
    grant(plugins, &id, &[], &[LibraryRead, TracksChanged]).await;
    enable_ungranted(db, "lrclib-lyrics").await;
    changes(db, change).await;
}

/// A track's change, which the lyrics plugin looks up.
const UPSERT: &str = "(1, 'track', 11, 'upsert', 0)";

/// The real lyrics plugin hears of a change in the feed as `tracks-changed`, finds it can't
/// look the track up without the network, and the failure is recorded to retry, with the
/// network it was refused and the compute it spent.
#[tokio::test]
async fn delivers_changes_to_a_real_plugin() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    lyrics(&plugins, &db, UPSERT).await;

    plugins.dispatch().await;
    let (report, failures, user) = delivered(&db).await;
    assert_eq!((failures, user), (1, 0), "one failure, to retry");
    assert_eq!(uncounted(report), format!("{LOOKUP_REFUSED} (refused Network access; "));
}

/// A removed track needs no lookup, so the real lyrics plugin handles it without the network,
/// and its run is recorded with the compute it spent and nothing refused.
#[tokio::test]
async fn a_real_plugin_that_succeeds_is_recorded_with_its_compute() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    lyrics(&plugins, &db, "(1, 'track', 11, 'delete', 0)").await;

    plugins.dispatch().await;
    let (report, failures, _) = delivered(&db).await;
    assert_eq!(failures, 0);
    assert_eq!(uncounted(report), "Found lyrics for 0 of 0 tracks (0 synced, 0 plain). (");
}

/// A fifth failure in a row disables the real plugin in the library, with its run's summary as
/// the reason. The deadline only bounds a hang.
#[tokio::test]
async fn a_fifth_failure_in_a_row_disables_a_real_plugin_with_why() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    lyrics(&plugins, &db, UPSERT).await;
    let failed = "INSERT INTO plugin_cursors (plugin_id, library_id, hook, failures) \
                  VALUES ('lrclib-lyrics', 1, 'tracksChanged', 4)";
    db.write(move |tx| tx.execute_batch(failed)).await.unwrap();

    plugins.dispatch().await;
    let sql = "SELECT disabled_reason FROM plugin_libraries WHERE disabled_reason IS NOT NULL";
    let disabled = async {
        loop {
            let reason = db.read(|conn| conn.query_row(sql, [], |row| row.get(0)).optional());
            match reason.await.unwrap() {
                Some(reason) => return reason,
                None => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    };
    let reason: String = tokio::time::timeout(Duration::from_secs(30), disabled).await.unwrap();
    assert_eq!(reason, format!("Stopped after failing 5 times in a row: {LOOKUP_REFUSED}"));
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
    let (report, failures, user) = delivered(&db).await;
    assert_eq!((failures, user), (1, 7), "kept for user 7 to send again");
    assert_eq!(uncounted(report), format!("{REFUSED} (refused Network access; "));
}
