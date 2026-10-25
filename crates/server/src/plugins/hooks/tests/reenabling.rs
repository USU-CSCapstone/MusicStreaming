//! Enabling a plugin in a library again starts its hooks there afresh: the failing that
//! may have disabled it is cleared, and a delivery under way counts from there.

use jewelcase_plugins::Permission::TracksChanged;

use super::super::due::all;
use super::super::{Ran, record};
use super::failing::{REFUSED, changed, enabled, hooks, hourly_failures};
use super::{HOUR, ran};
use crate::plugins::testing::{enable, plugins};

/// Re-enabling a plugin disabled for failing starts it afresh: what waited to be retried is due
/// at once, and it takes 72 hours of failing to disable it again.
#[tokio::test]
async fn re_enabling_a_plugin_starts_its_failing_over() {
    let (temp, db) = changed().await;
    hourly_failures(&db, 0, 72).await;
    assert!(!enabled(&db).await);

    enable(&plugins(&temp, &db), "plugin", true).await;
    let now = 72 * HOUR + 1;
    assert_eq!(hooks(ran(&db, now, Ran::Failed, REFUSED).await), [TracksChanged], "due at once");
    assert!(enabled(&db).await, "an instant of failing since it was enabled again");
    hourly_failures(&db, 73, 73).await;
    assert!(enabled(&db).await, "an hour of failing since it was enabled again");
}

/// Enabling it in library 1 leaves what it failed in library 2 as it was.
#[tokio::test]
async fn enabling_in_one_library_leaves_the_failing_in_another() {
    let (temp, db) = changed().await;
    let failed = "INSERT INTO plugin_cursors (plugin_id, library_id, hook, failing_since, retry_at) \
                  VALUES ('plugin', 2, 'tracksChanged', 3, 99)";
    db.write(move |tx| tx.execute(failed, [])).await.unwrap();
    enable(&plugins(&temp, &db), "plugin", true).await;
    let sql = "SELECT failing_since, retry_at FROM plugin_cursors WHERE library_id = 2";
    let kept: (i64, Option<i64>) = db
        .read(move |conn| conn.query_row(sql, [], |row| Ok((row.get(0)?, row.get(1)?))))
        .await
        .unwrap();
    assert_eq!(kept, (3, Some(99)));
}

/// A delivery found due before the plugin was enabled again, and failing after, counts from
/// the fresh start rather than from the failing before it.
#[tokio::test]
async fn a_delivery_under_way_counts_from_the_fresh_start() {
    let (temp, db) = changed().await;
    hourly_failures(&db, 0, 71).await;
    let now = 72 * HOUR;
    let found = db.read(move |conn| all(conn, now)).await.unwrap();
    assert_eq!(found.len(), 1);

    enable(&plugins(&temp, &db), "plugin", true).await;
    db.write(move |tx| record(tx, &found[0], 0, Ran::Failed, REFUSED, now)).await.unwrap();
    assert!(enabled(&db).await, "an instant of failing since it was enabled again");
}
