//! The change hooks wake only for changes plugins can see, however the feed coalesces them with
//! changes plugins can't see, while devices still hear of every change.

use jewelcase_plugins::Event;
use jewelcase_plugins::Permission::{LibraryRead, TracksChanged};

use super::{HANDLED, deliver, plan, setup};
use crate::db::Database;
use crate::db::feed::{self, Entity, Op};
use crate::plugins::testing::grant;

/// A change to track `?1` that plugins see: its title.
const SEEN: &str = "UPDATE tracks SET title = title || '!' WHERE id = ?1";
/// A change to track `?1` that only devices see: its loudness.
const UNSEEN: &str = "UPDATE tracks SET loudness_lufs = ifnull(loudness_lufs, 0) - 1 WHERE id = ?1";

/// Makes the change `sql` to track 11 and records it on the feed, as a write site does, and
/// returns its feed position for devices.
async fn change(db: &Database, sql: &'static str) -> i64 {
    change_track(db, 11, sql).await
}

/// Makes the change `sql` to track `track`, named `?1` in it, and records it on the feed, as a
/// write site does, and returns its feed position for devices.
async fn change_track(db: &Database, track: i64, sql: &'static str) -> i64 {
    db.write(move |tx| {
        tx.execute(sql, [track])?;
        feed::record(tx, 1, Entity::Track, track, Op::Upsert, 0)?;
        tx.query_row(
            "SELECT seq FROM library_changes WHERE entity_type = 'track' AND entity_id = ?1",
            [track],
            |row| row.get(0),
        )
    })
    .await
    .unwrap()
}

/// The tracks a delivery at time 0 names as changed, from each event it delivered.
async fn delivered(db: &Database) -> Vec<Vec<u64>> {
    deliver(db, 0, HANDLED)
        .await
        .into_iter()
        .map(|r| match r.event {
            Some(Event::TracksChanged(changes)) => changes.changed,
            other => panic!("not tracks changed: {other:?}"),
        })
        .collect()
}

/// A plugin given `tracksChanged` in library 1, with track 11 changed in a way it sees.
async fn seen_once() -> (tempfile::TempDir, std::sync::Arc<Database>) {
    let (temp, db, plugins) = setup().await;
    grant(&plugins, "plugin", &[], &[LibraryRead, TracksChanged]).await;
    change(&db, SEEN).await;
    (temp, db)
}

/// A change plugins can't see, after they have handled one they can, wakes nothing, though
/// devices hear of it.
#[tokio::test]
async fn an_unseen_change_after_the_plugin_caught_up_wakes_nothing() {
    let (_temp, db) = seen_once().await;
    assert_eq!(delivered(&db).await, [vec![11]]);
    let before = change(&db, SEEN).await;
    assert_eq!(delivered(&db).await, [vec![11]], "a change it sees wakes it again");

    let after = change(&db, UNSEEN).await;
    assert!(after > before, "devices still hear of it");
    assert!(delivered(&db).await.is_empty(), "the plugin is not woken");
}

/// A batch due for one track's change plugins see leaves out another track's change they can't
/// see, made in the same window.
#[tokio::test]
async fn a_batch_names_only_the_changes_plugins_see() {
    let (_temp, db) = seen_once().await;
    assert_eq!(delivered(&db).await, [vec![11]]);
    change_track(&db, 11, UNSEEN).await;
    change_track(&db, 12, SEEN).await;
    assert_eq!(delivered(&db).await, [vec![12]]);
}

/// A change plugins can't see, coalesced over one they can that they have not handled yet,
/// still delivers that one, once.
#[tokio::test]
async fn an_unseen_change_never_hides_a_seen_one_not_yet_delivered() {
    let (_temp, db) = seen_once().await;
    change(&db, UNSEEN).await;
    assert_eq!(delivered(&db).await, [vec![11]]);
    assert!(delivered(&db).await.is_empty(), "once");
}

/// A change plugins see, coalesced over one they can't, wakes them.
#[tokio::test]
async fn a_seen_change_after_an_unseen_one_wakes_the_plugin() {
    let (_temp, db) = seen_once().await;
    assert_eq!(delivered(&db).await, [vec![11]]);
    change(&db, UNSEEN).await;
    change(&db, SEEN).await;
    assert_eq!(delivered(&db).await, [vec![11]]);
}

/// A track that leaves what plugins can read, and comes back, wakes them each time, and a change
/// they can't see while it is gone wakes nothing.
#[tokio::test]
async fn a_track_going_missing_and_returning_wakes_the_plugin() {
    let (_temp, db) = seen_once().await;
    assert_eq!(delivered(&db).await, [vec![11]]);
    change(&db, "UPDATE tracks SET missing_since = 1 WHERE id = ?1").await;
    assert_eq!(delivered(&db).await, [vec![11]], "gone from get-tracks");
    change(&db, UNSEEN).await;
    assert!(delivered(&db).await.is_empty(), "still gone");
    change(&db, "UPDATE tracks SET missing_since = NULL WHERE id = ?1").await;
    assert_eq!(delivered(&db).await, [vec![11]], "back");
}

/// A file rewritten with the same tags wakes a plugin, which can read the file.
#[tokio::test]
async fn a_rewritten_file_wakes_the_plugin() {
    let (_temp, db) = seen_once().await;
    assert_eq!(delivered(&db).await, [vec![11]]);
    change(&db, "UPDATE tracks SET file_mtime = file_mtime + 1 WHERE id = ?1").await;
    assert_eq!(delivered(&db).await, [vec![11]]);
}

/// A track deleted after plugins heard it went missing reaches them as removed, though what they
/// can see of it, nothing, is as it was.
#[tokio::test]
async fn a_deletion_always_reaches_the_plugin() {
    let (_temp, db) = seen_once().await;
    assert_eq!(delivered(&db).await, [vec![11]]);
    change(&db, "UPDATE tracks SET missing_since = 1 WHERE id = ?1").await;
    assert_eq!(delivered(&db).await, [vec![11]], "gone from get-tracks");
    db.write(|tx| {
        tx.execute("DELETE FROM tracks WHERE id = 11", [])?;
        feed::record(tx, 1, Entity::Track, 11, Op::Delete, 0)
    })
    .await
    .unwrap();
    let removed: Vec<_> = deliver(&db, 0, HANDLED)
        .await
        .into_iter()
        .map(|r| match r.event {
            Some(Event::TracksChanged(changes)) => (changes.changed, changes.removed),
            other => panic!("not tracks changed: {other:?}"),
        })
        .collect();
    assert_eq!(removed, [(vec![], vec![11])]);
}

/// Both the query that finds a change hook due and the one that reads its batch walk one kind
/// of change by the position plugins see, through the index on it.
#[tokio::test]
async fn change_hooks_read_plugin_positions_through_their_index() {
    let (_temp, db, _plugins) = setup().await;
    let index = "USING INDEX library_changes_plugin_feed \
                 (library_id=? AND entity_type=? AND plugin_seq>?)";
    let steps = plan(&db, TracksChanged).await;
    assert!(steps.iter().any(|step| step.ends_with(index)), "due: {steps:#?}");

    let steps: Vec<String> = db
        .read(|conn| {
            // `batch`'s query.
            let sql = "EXPLAIN QUERY PLAN SELECT plugin_seq, entity_id, op FROM library_changes \
                       WHERE library_id = 1 AND entity_type = 'track' AND plugin_seq > 0 \
                       ORDER BY plugin_seq LIMIT 100";
            conn.prepare(sql)?.query_map([], |row| row.get(3))?.collect()
        })
        .await
        .unwrap();
    assert!(steps.iter().all(|step| !step.contains("TEMP B-TREE")), "batch: {steps:#?}");
    assert!(steps.iter().any(|step| step.ends_with(index)), "batch: {steps:#?}");
}
