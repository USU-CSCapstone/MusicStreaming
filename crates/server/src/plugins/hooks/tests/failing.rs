//! A failing delivery is tried again after as long as the plugin has been failing, from a minute
//! to an hour, whatever the cause, and disables the plugin once it has failed for 72 hours with
//! no success between.

use std::sync::Arc;

use jewelcase_plugins::Permission;
use jewelcase_plugins::Permission::{LibraryRead, TracksChanged};

use super::super::{BACKOFF_MS, MAX_BACKOFF_MS, Ran};
use super::{HOUR, Received, changes, ran, setup};
use crate::db::Database;
use crate::plugins::testing::grant;

/// The summary of a failed run.
pub(super) const REFUSED: &str = "LRCLIB answered 400";

/// The hooks `received` delivered.
pub(super) fn hooks(received: Vec<Received>) -> Vec<Permission> {
    received.into_iter().map(|r| r.hook).collect()
}

/// Whether the plugin is still enabled in library 1.
pub(super) async fn enabled(db: &Database) -> bool {
    db.read(|conn| conn.query_row("SELECT enabled FROM plugin_libraries", [], |row| row.get(0)))
        .await
        .unwrap()
}

/// Why the plugin was disabled in library 1, if it was.
async fn reason(db: &Database) -> Option<String> {
    let sql = "SELECT disabled_reason FROM plugin_libraries";
    db.read(move |conn| conn.query_row(sql, [], |row| row.get(0))).await.unwrap()
}

/// The plugin granted `tracksChanged` in library 1, with a change to deliver.
pub(super) async fn changed() -> (tempfile::TempDir, Arc<Database>) {
    let (temp, db, plugins) = setup().await;
    grant(&plugins, "plugin", &[], &[LibraryRead, TracksChanged]).await;
    changes(&db, "(1, 'track', 11, 'upsert', 0)").await;
    (temp, db)
}

/// Delivers a failure on the hour, each hour from `first` to `last`, each one due by then.
pub(super) async fn hourly_failures(db: &Database, first: i64, last: i64) {
    for hour in first..=last {
        let tried = hooks(ran(db, hour * HOUR, Ran::Failed, REFUSED).await);
        assert_eq!(tried, [TracksChanged], "hour {hour}");
    }
}

#[tokio::test]
async fn a_failing_hook_waits_as_long_as_it_has_been_failing_up_to_an_hour() {
    let (_temp, db) = changed().await;
    let tried = [TracksChanged];
    let fail = async |at| ran(&db, at, Ran::Failed, REFUSED).await;

    assert_eq!(hooks(fail(0).await), tried);
    assert!(fail(BACKOFF_MS - 1).await.is_empty(), "a minute");
    assert_eq!(hooks(fail(BACKOFF_MS).await), tried);
    assert!(fail(2 * BACKOFF_MS - 1).await.is_empty(), "a minute again, failing for one");
    assert_eq!(hooks(fail(2 * BACKOFF_MS).await), tried);
    assert!(fail(4 * BACKOFF_MS - 1).await.is_empty(), "two minutes, failing for two");
    assert_eq!(hooks(fail(4 * BACKOFF_MS).await), tried);
    assert!(fail(8 * BACKOFF_MS - 1).await.is_empty(), "four minutes");

    // Failing for ten hours, the wait is the hour it is capped at.
    let late = 10 * HOUR;
    assert_eq!(hooks(fail(late).await), tried);
    assert!(fail(late + MAX_BACKOFF_MS - 1).await.is_empty(), "capped at an hour");
    assert_eq!(hooks(fail(late + MAX_BACKOFF_MS).await), tried);
    assert!(enabled(&db).await);
}

#[tokio::test]
async fn a_hook_that_fails_for_72_hours_disables_the_plugin() {
    // Begun a millisecond in, so the last is 72 hours less a millisecond after it.
    let (_temp, db) = changed().await;
    assert_eq!(hooks(ran(&db, 1, Ran::Failed, REFUSED).await), [TracksChanged]);
    hourly_failures(&db, 1, 72).await;
    assert!(enabled(&db).await, "just under 72 hours");

    let (_temp, db) = changed().await;
    hourly_failures(&db, 0, 71).await;
    assert!(enabled(&db).await);
    hourly_failures(&db, 72, 72).await;
    assert!(!enabled(&db).await, "72 hours");
    let why = "Stopped after 72 hours of failing: LRCLIB answered 400";
    assert_eq!(reason(&db).await.as_deref(), Some(why));
}

#[tokio::test]
async fn a_success_clears_the_failing_so_the_clock_and_the_wait_start_again() {
    let (_temp, db) = changed().await;
    hourly_failures(&db, 0, 71).await;
    assert_eq!(hooks(ran(&db, 72 * HOUR, Ran::Handled, "").await), [TracksChanged]);
    changes(&db, "(1, 'track', 12, 'upsert', 0)").await;

    // Failing again waits only the first backoff, and takes 72 hours more to disable.
    let begun = 72 * HOUR;
    assert_eq!(hooks(ran(&db, begun, Ran::Failed, REFUSED).await), [TracksChanged]);
    assert!(ran(&db, begun + BACKOFF_MS - 1, Ran::Failed, REFUSED).await.is_empty());
    assert_eq!(hooks(ran(&db, begun + BACKOFF_MS, Ran::Failed, REFUSED).await), [TracksChanged]);
    hourly_failures(&db, 73, 72 + 71).await;
    assert!(enabled(&db).await, "71 hours since it began again");
    hourly_failures(&db, 72 + 72, 72 + 72).await;
    assert!(!enabled(&db).await, "72 hours since it began again");
}
