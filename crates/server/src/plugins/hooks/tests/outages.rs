//! A delivery that fails after the plugin found a service out of reach is tried again like any
//! failure, but never counts toward the failures that disable the plugin. Only an outage that
//! lasts 72 hours, with no success or other failure between, disables it.

use std::sync::Arc;

use jewelcase_plugins::Permission;
use jewelcase_plugins::Permission::{LibraryRead, TracksChanged};

use super::super::{BACKOFF_MS, MAX_FAILURES, Ran};
use super::{HOUR, Received, changes, ran, setup};
use crate::db::Database;
use crate::plugins::testing::grant;

/// The summary of a run that found its service out of reach.
const TIMED_OUT: &str = "the request timed out";
/// The summary of a run that failed on its own.
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

/// Delivers an outage on the hour, each hour from `first` to `last`, each one due by then.
pub(super) async fn hourly_outages(db: &Database, first: i64, last: i64) {
    for hour in first..=last {
        let tried = hooks(ran(db, hour * HOUR, Ran::Unreachable, TIMED_OUT).await);
        assert_eq!(tried, [TracksChanged], "hour {hour}");
    }
}

#[tokio::test]
async fn outages_back_off_but_never_disable_the_plugin_and_other_failures_still_count() {
    let (_temp, db) = changed().await;
    let tried = [TracksChanged];

    assert_eq!(hooks(ran(&db, 0, Ran::Unreachable, TIMED_OUT).await), tried);
    assert!(ran(&db, BACKOFF_MS - 1, Ran::Unreachable, TIMED_OUT).await.is_empty(), "waiting");
    assert_eq!(hooks(ran(&db, BACKOFF_MS, Ran::Unreachable, TIMED_OUT).await), tried);
    let third = 3 * BACKOFF_MS;
    assert!(ran(&db, third - 1, Ran::Unreachable, TIMED_OUT).await.is_empty(), "twice as long");
    let mut now = third;
    for attempt in 3..=2 * MAX_FAILURES {
        assert_eq!(hooks(ran(&db, now, Ran::Unreachable, TIMED_OUT).await), tried, "{attempt}");
        now += HOUR;
    }
    assert!(enabled(&db).await, "outages alone never disable it");

    // Failures of its own still count, with outages between them or not.
    for failure in 1..MAX_FAILURES {
        assert_eq!(hooks(ran(&db, now, Ran::Failed, REFUSED).await), tried, "{failure}");
        assert_eq!(hooks(ran(&db, now + HOUR, Ran::Unreachable, TIMED_OUT).await), tried);
        now += 2 * HOUR;
    }
    assert!(enabled(&db).await, "four failures of its own");
    assert_eq!(hooks(ran(&db, now, Ran::Failed, REFUSED).await), tried);
    assert!(!enabled(&db).await, "the fifth failure of its own in a row");
}

#[tokio::test]
async fn an_outage_disables_the_plugin_once_it_has_lasted_72_hours() {
    // Begun a millisecond in, so the last is 72 hours less a millisecond after it.
    let (_temp, db) = changed().await;
    assert_eq!(hooks(ran(&db, 1, Ran::Unreachable, TIMED_OUT).await), [TracksChanged]);
    hourly_outages(&db, 1, 72).await;
    assert!(enabled(&db).await, "just under 72 hours");

    let (_temp, db) = changed().await;
    hourly_outages(&db, 0, 71).await;
    assert!(enabled(&db).await);
    hourly_outages(&db, 72, 72).await;
    assert!(!enabled(&db).await, "72 hours");
    let why = "Stopped after 72 hours of being unable to run or reach its service: \
               the request timed out";
    assert_eq!(reason(&db).await.as_deref(), Some(why));
}

#[tokio::test]
async fn a_success_or_a_failure_of_its_own_starts_the_outage_clock_again() {
    let (_temp, db) = changed().await;
    hourly_outages(&db, 0, 40).await;
    assert_eq!(hooks(ran(&db, 41 * HOUR, Ran::Handled, "").await), [TracksChanged]);
    changes(&db, "(1, 'track', 12, 'upsert', 0)").await;
    // The success cleared the outages, so the next failure waits only the first backoff.
    let failed = hooks(ran(&db, 42 * HOUR, Ran::Failed, REFUSED).await);
    assert_eq!(failed, [TracksChanged]);
    let first_wait = 42 * HOUR + BACKOFF_MS;
    assert!(ran(&db, first_wait - 1, Ran::Unreachable, TIMED_OUT).await.is_empty());
    assert_eq!(hooks(ran(&db, first_wait, Ran::Unreachable, TIMED_OUT).await), [TracksChanged]);
    hourly_outages(&db, 43, 42 + 71).await;
    assert!(enabled(&db).await, "under 71 hours since the outage began");

    // A failure of its own shows the service was reached, or that the plugin is at fault. The
    // outages before it still set its backoff, which is an hour by now.
    let failed = hooks(ran(&db, 114 * HOUR, Ran::Failed, REFUSED).await);
    assert_eq!(failed, [TracksChanged]);
    assert!(ran(&db, 114 * HOUR + 2 * BACKOFF_MS, Ran::Failed, REFUSED).await.is_empty());
    hourly_outages(&db, 115, 115 + 71).await;
    assert!(enabled(&db).await, "71 hours since the failure");
    hourly_outages(&db, 115 + 72, 115 + 72).await;
    assert!(!enabled(&db).await, "72 hours since the failure");
}
