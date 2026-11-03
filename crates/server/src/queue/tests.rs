//! The queue store and its background save. Tests drive `save` directly, never the timer.

use std::collections::HashMap;
use std::sync::Arc;

use jewelcase_core::{Anchor, Edit, QueueState, Repeat, Source};
use rusqlite::Connection;
use serde_json::json;

use super::hash::{list_bytes, list_hash};
use super::keep::{KeepError, NewQueue, fresh};
use super::store::{Account, Position, Queues};
use crate::api::Uuid;
use crate::api::testing::{add_user, conn};
use crate::db::Database;
use crate::db::testing::open;

const QUEUE: &str = "11111111-1111-4111-8111-111111111111";
const OTHER: &str = "22222222-2222-4222-8222-222222222222";
const THIRD: &str = "33333333-3333-4333-8333-333333333333";

/// A database with accounts 1 and 2, and a connection to look at it with.
fn setup() -> (tempfile::TempDir, Arc<Database>, Connection) {
    let (temp, db) = open();
    let conn = conn(&temp);
    add_user(&conn, 1, "admin");
    add_user(&conn, 2, "user");
    (temp, db, conn)
}

/// A queue state that plays `tracks`, with the list attached, which hashes as `hash`.
fn state_over(key: &str, hash: &str, tracks: &Arc<[u64]>) -> QueueState {
    let mut state = QueueState::default();
    state.start(Source::new(key, "1", tracks.len() as u64, None), 0).unwrap();
    state.attach_list(key, hash.to_owned(), tracks.clone()).unwrap();
    state
}

/// An account playing `tracks`, which it holds. Returns the list's hash too.
fn playing(queue: &str, tracks: &[u64]) -> (Account, String) {
    let tracks: Arc<[u64]> = tracks.into();
    let mut account = Account::new(
        Uuid::try_from(queue.to_owned()).unwrap(),
        json!({ "type": "album" }),
        QueueState::default(),
    );
    let hash = account.hold(1, tracks.clone());
    account.state = state_over("a", &hash, &tracks);
    (account, hash)
}

/// `Queues` holding `accounts`.
fn holding(accounts: impl IntoIterator<Item = (i64, Account)>) -> Arc<Queues> {
    Arc::new(Queues::holding(HashMap::from_iter(accounts)))
}

/// The hashes in `queue_lists` for `user`, sorted.
fn stored(conn: &Connection, user: i64) -> Vec<String> {
    let mut select =
        conn.prepare("SELECT hash FROM queue_lists WHERE user_id = ? ORDER BY hash").unwrap();
    select.query_map([user], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
}

/// The hashes in `queue_list_uses` for `user`, sorted.
fn uses(conn: &Connection, user: i64) -> Vec<String> {
    let mut select =
        conn.prepare("SELECT hash FROM queue_list_uses WHERE user_id = ? ORDER BY hash").unwrap();
    select.query_map([user], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
}

#[test]
fn the_hash_is_sha256_of_little_endian_ids() {
    assert_eq!(list_hash(&[]), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    let tracks = [1, 0x0102030405060708];
    assert_eq!(
        list_hash(&tracks),
        "c171c8538d4164ae2e076f90f5b54f4d1ad245555e62a7902d4c8b7f0b193f21"
    );
    assert_eq!(list_bytes(&tracks), [1, 0, 0, 0, 0, 0, 0, 0, 8, 7, 6, 5, 4, 3, 2, 1]);
}

#[tokio::test]
async fn active_queue_survives_restart() {
    let (_temp, db, conn) = setup();
    let (mut account, hash) = playing(QUEUE, &[5, 6, 7]);
    account.version = 4;
    let queues = holding([(1, account)]);
    queues.save(&db).await;

    // A parked queue is not loaded: only the active one is.
    conn.execute(
        "INSERT INTO queues (user_id, queue_id, version, context, state, active, last_active) \
         VALUES (2, ?1, 0, '{}', ?2, 0, 1)",
        [OTHER, &serde_json::to_string(&QueueState::default()).unwrap()],
    )
    .unwrap();

    let restarted = Queues::load(&db).await.unwrap();
    let accounts = restarted.lock();
    assert_eq!(accounts.len(), 1);
    let loaded = &accounts[&1];
    let saved = &queues.lock()[&1];
    assert_eq!((loaded.queue_id.as_str(), loaded.version), (QUEUE, 4));
    assert_eq!(loaded.context, json!({ "type": "album" }));
    assert_eq!(
        serde_json::to_value(&loaded.state).unwrap(),
        serde_json::to_value(&saved.state).unwrap()
    );
    assert!(!loaded.changed && loaded.lists.is_empty());
    // The core runs with no list attached; the list waits in the database.
    assert!(loaded.state.sources[0].tracks().is_none());
    let (bytes, length, library): (Vec<u8>, u64, i64) = conn
        .query_row(
            "SELECT tracks, length, library_id FROM queue_lists WHERE user_id = 1 AND hash = ?1",
            [&hash],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!((bytes, length, library), (list_bytes(&[5, 6, 7]), 3, 1));
    assert_eq!(uses(&conn, 1), [hash]);
}

#[tokio::test]
async fn list_added_and_removed_is_never_written() {
    let (_temp, db, conn) = setup();
    let (mut account, _) = playing(QUEUE, &[1, 2]);
    // Held for an edit that was undone before any save.
    let dropped = account.hold(1, [8, 9].into());
    account.changed = true;
    let queues = holding([(1, account)]);
    queues.save(&db).await;

    assert!(!stored(&conn, 1).contains(&dropped));
    assert_eq!(stored(&conn, 1).len(), 1);
    // And memory has let go of both: one was stored, the other is unused.
    assert!(queues.lock()[&1].lists.is_empty());
}

#[tokio::test]
async fn unused_list_deleted_on_save() {
    let (_temp, db, conn) = setup();
    let (first, old) = playing(QUEUE, &[1, 2]);
    // Account 2 uses the same list as account 1: each account has its own copy.
    let queues = holding([(1, first), (2, playing(QUEUE, &[1, 2]).0)]);
    queues.save(&db).await;
    assert_eq!((stored(&conn, 1), stored(&conn, 2)), (vec![old.clone()], vec![old.clone()]));

    // Account 1's queue moves on to another list; account 2's is untouched.
    let tracks: Arc<[u64]> = [3].into();
    {
        let mut accounts = queues.lock();
        let account = accounts.get_mut(&1).unwrap();
        let hash = account.hold(1, tracks.clone());
        account.state = state_over("b", &hash, &tracks);
        account.changed = true;
    }
    queues.save(&db).await;
    let kept = list_hash(&[3]);
    assert_eq!((stored(&conn, 1), uses(&conn, 1)), (vec![kept.clone()], vec![kept]));
    assert_eq!(stored(&conn, 2), [old]);
}

#[tokio::test]
async fn list_readded_during_save_survives() {
    let (_temp, db, conn) = setup();
    let (account, hash) = playing(QUEUE, &[1, 2]);
    let queues = holding([(1, account)]);
    queues.save(&db).await;
    assert_eq!(stored(&conn, 1), vec![hash.clone()]);

    // The queue drops the list, and the save that writes that is under way when it comes back.
    {
        let mut accounts = queues.lock();
        let account = accounts.get_mut(&1).unwrap();
        account.state = QueueState::default();
        account.changed = true;
    }
    let saving = queues.clone();
    let saved = db
        .write(move |tx| {
            let saved = saving.write_changed(tx);
            let mut accounts = saving.lock();
            let account = accounts.get_mut(&1).unwrap();
            let tracks: Arc<[u64]> = [1, 2].into();
            assert_eq!(account.hold(1, tracks.clone()), hash);
            account.state = state_over("a", &hash, &tracks);
            account.changed = true;
            Ok(saved)
        })
        .await
        .unwrap();
    // The save deleted the unused list, and nothing in the database has it now.
    assert!(stored(&conn, 1).is_empty());
    queues.committed(saved);
    assert_eq!(queues.lock()[&1].lists.len(), 1);

    queues.save(&db).await;
    let hash = list_hash(&[1, 2]);
    assert_eq!((stored(&conn, 1), uses(&conn, 1)), (vec![hash.clone()], vec![hash]));
    assert!(queues.lock()[&1].lists.is_empty());
}

#[tokio::test]
async fn failed_savepoint_is_per_account() {
    let (_temp, db, conn) = setup();
    let (good, kept) = playing(QUEUE, &[1, 2]);
    let (bad, held) = playing(OTHER, &[3]);
    // Writing account 2's queue fails after its list went in.
    conn.execute_batch(
        "CREATE TRIGGER refuse BEFORE INSERT ON queues WHEN NEW.user_id = 2 \
         BEGIN SELECT RAISE(ABORT, 'refused'); END",
    )
    .unwrap();
    let queues = holding([(1, good), (2, bad)]);
    queues.save(&db).await;

    // Account 1 is saved. Account 2's list went in before the clash and came out with it.
    assert_eq!(stored(&conn, 1), [kept]);
    assert!(stored(&conn, 2).is_empty() && uses(&conn, 2).is_empty());
    {
        let accounts = queues.lock();
        assert!(!accounts[&1].changed && accounts[&1].lists.is_empty());
        assert!(accounts[&2].changed && accounts[&2].lists.contains_key(&held));
    }
    let rows: u32 =
        conn.query_row("SELECT count(*) FROM queues WHERE user_id = 2", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 0);

    // Once it can be written, the next save does.
    conn.execute_batch("DROP TRIGGER refuse").unwrap();
    queues.save(&db).await;
    assert_eq!((stored(&conn, 2), uses(&conn, 2)), (vec![held.clone()], vec![held]));
    assert!(!queues.lock()[&2].changed);
}

#[tokio::test]
async fn saving_an_unchanged_account_writes_nothing() {
    let (_temp, db, conn) = setup();
    let (account, hash) = playing(QUEUE, &[1]);
    let queues = holding([(1, account)]);
    queues.save(&db).await;
    conn.execute("UPDATE queues SET version = 9", []).unwrap();
    queues.save(&db).await;
    let version: u32 = conn.query_row("SELECT version FROM queues", [], |row| row.get(0)).unwrap();
    assert_eq!((version, stored(&conn, 1)), (9, vec![hash]));
}

#[tokio::test]
async fn a_list_the_database_already_has_is_held_again_without_failing() {
    let (_temp, db, conn) = setup();
    let (account, hash) = playing(QUEUE, &[1, 2]);
    let queues = holding([(1, account)]);
    queues.save(&db).await;

    // Dropped and added again within a second of its first save, so held although stored.
    {
        let mut accounts = queues.lock();
        let account = accounts.get_mut(&1).unwrap();
        assert_eq!(account.hold(1, [1, 2].into()), hash);
        account.version = 1;
        account.changed = true;
    }
    queues.save(&db).await;
    let version: u32 = conn.query_row("SELECT version FROM queues", [], |row| row.get(0)).unwrap();
    assert_eq!((version, stored(&conn, 1)), (1, vec![hash]));
}

#[tokio::test]
async fn an_edit_is_saved_over_the_last_save_and_reloads() {
    let (_temp, db, _conn) = setup();
    let (account, _) = playing(QUEUE, &[1, 2, 3]);
    let queues = holding([(1, account)]);
    queues.save(&db).await;
    {
        let mut accounts = queues.lock();
        let account = accounts.get_mut(&1).unwrap();
        account.version = 5;
        account.context = json!({ "type": "search", "q": "x" });
        account.state.apply(&jewelcase_core::Edit::Next).unwrap();
        account.changed = true;
    }
    queues.save(&db).await;

    let restarted = Queues::load(&db).await.unwrap();
    let accounts = restarted.lock();
    let (loaded, saved) = (&accounts[&1], &queues.lock()[&1]);
    assert_eq!(loaded.version, 5);
    assert_eq!(loaded.context, json!({ "type": "search", "q": "x" }));
    assert_eq!(
        serde_json::to_value(&loaded.state).unwrap(),
        serde_json::to_value(&saved.state).unwrap()
    );
    assert_eq!(loaded.state.next_play, saved.state.next_play);
    assert!(loaded.state.next_play > 0);
}

#[tokio::test]
async fn an_unreadable_active_row_does_not_keep_a_new_queue_from_saving() {
    let (_temp, db, conn) = setup();
    conn.execute(
        "INSERT INTO queues (user_id, queue_id, version, context, state, active) \
         VALUES (1, ?1, 0, '{}', '{}', 1)",
        [OTHER],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO queues (user_id, queue_id, version, context, state, active) \
         VALUES (2, ?1, 0, '{}', '{}', 1)",
        [OTHER],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO queues (user_id, queue_id, version, context, state, active, last_active) \
         VALUES (1, 'kept', 0, '{}', '{}', 0, 1)",
        [],
    )
    .unwrap();
    // Not a queue state, so it is not loaded.
    assert!(Queues::load(&db).await.unwrap().lock().is_empty());

    let (account, hash) = playing(QUEUE, &[1]);
    let queues = holding([(1, account)]);
    queues.save(&db).await;
    assert_eq!(stored(&conn, 1), [hash]);
    assert!(!queues.lock()[&1].changed);
    let active: Vec<String> = conn
        .prepare("SELECT queue_id FROM queues WHERE active = 1 AND user_id = 1")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(active, [QUEUE]);
    let parked: u32 = conn
        .query_row(
            "SELECT count(*) FROM queues WHERE active = 0 AND last_active IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(parked, 2);
    let untouched: (i64, i64) = (
        conn.query_row(
            "SELECT count(*) FROM queues WHERE user_id = 2 AND active = 1 AND last_active IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap(),
        conn.query_row("SELECT last_active FROM queues WHERE queue_id = 'kept'", [], |r| r.get(0))
            .unwrap(),
    );
    assert_eq!(untouched, (1, 1));
}

// ───────────────────────── parked and kept queues ─────────────────────────

/// `queue` as a queue ID. Panics unless it is a UUID.
fn id(queue: &str) -> Uuid {
    Uuid::try_from(queue.to_owned()).unwrap()
}

/// A queue to start over `tracks`, whose source is keyed `k<queue>`.
fn new_queue(queue: &str, tracks: &[u64]) -> NewQueue {
    NewQueue {
        queue_id: id(queue),
        context: json!({ "type": "album" }),
        key: format!("k{queue}"),
        library_id: 1,
        tracks: tracks.into(),
        recipe: None,
        at: 0,
    }
}

/// Starts `queue` over `tracks` for account 1, as a device does, with no player state.
async fn start(
    queues: &Arc<Queues>,
    db: &Database,
    queue: &str,
    tracks: &[u64],
) -> Result<(), KeepError> {
    queues.start(db, 1, new_queue(queue, tracks), || None).await
}

/// Marks account 1's active queue adjusted, as a queued track would.
fn adjust(queues: &Queues) {
    let mut accounts = queues.lock();
    let account = accounts.get_mut(&1).unwrap();
    account.state.adjusted = true;
    account.changed = true;
}

/// Adds a second list to account 1's active queue at the end, as Queue Last does, holding it.
fn add_list(queues: &Queues, key: &str, tracks: &[u64]) -> String {
    let mut accounts = queues.lock();
    let account = accounts.get_mut(&1).unwrap();
    let hash = account.hold(1, tracks.into());
    let mut source = Source::new(key, "1", tracks.len() as u64, None);
    source.hash = Some(hash.clone());
    account.state.apply(&Edit::Add { source, before: Anchor::ManualEnd }).unwrap();
    account.changed = true;
    hash
}

/// `(active, last_active, position)` of account 1's `queue`, if it has a row.
fn row(conn: &Connection, queue: &str) -> Option<(i64, Option<i64>, Option<String>)> {
    conn.query_row(
        "SELECT active, last_active, position FROM queues WHERE user_id = 1 AND queue_id = ?1",
        [queue],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .ok()
}

/// The IDs of account 1's queues with the given `active`, sorted.
fn queue_ids(conn: &Connection, active: i64) -> Vec<String> {
    let mut select = conn
        .prepare("SELECT queue_id FROM queues WHERE user_id = 1 AND active = ?1 ORDER BY queue_id")
        .unwrap();
    select.query_map([active], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
}

/// The hashes queue `queue` of account 1 uses, sorted.
fn uses_of(conn: &Connection, queue: &str) -> Vec<String> {
    let mut select = conn
        .prepare(
            "SELECT hash FROM queue_list_uses WHERE user_id = 1 AND queue_id = ?1 ORDER BY hash",
        )
        .unwrap();
    select.query_map([queue], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
}

/// `hashes` in order, to compare lists regardless of the order the database returned them.
fn sorted(mut hashes: Vec<String>) -> Vec<String> {
    hashes.sort();
    hashes
}

/// Account 1 playing adjusted queue `A` over `[1, 2]`, saved, and the list's hash.
async fn adjusted_and_saved(db: &Database) -> (Arc<Queues>, String) {
    let (account, hash) = playing(QUEUE, &[1, 2]);
    let queues = holding([(1, account)]);
    adjust(&queues);
    queues.save(db).await;
    (queues, hash)
}

/// A position `position_ms` into `item`, as a device reports it.
fn at(item: &str, position_ms: u32) -> Option<Position> {
    Some(Position { item: item.into(), position_ms })
}

#[tokio::test]
async fn start_within_second_of_add_keeps_list() {
    let (_temp, db, conn) = setup();
    let (queues, first) = adjusted_and_saved(&db).await;
    // A list is added, and a start parks the queue before any save sees the add.
    let added = add_list(&queues, "b", &[7, 8]);
    start(&queues, &db, OTHER, &[9]).await.unwrap();
    queues.save(&db).await;

    let third = list_hash(&[9]);
    assert_eq!(stored(&conn, 1), sorted(vec![first.clone(), added.clone(), third.clone()]));
    assert_eq!(uses_of(&conn, QUEUE), sorted(vec![first, added]));
    assert_eq!(
        (queue_ids(&conn, 0), queue_ids(&conn, 1)),
        (vec![QUEUE.to_owned()], vec![OTHER.to_owned()])
    );
    assert!(queues.lock()[&1].lists.is_empty());
}

#[tokio::test]
async fn untouched_queue_deleted_with_lists() {
    let (_temp, db, conn) = setup();
    let (account, first) = playing(QUEUE, &[1, 2]);
    let queues = holding([(1, account)]);
    queues.save(&db).await;
    assert_eq!((stored(&conn, 1), queue_ids(&conn, 1)), (vec![first], vec![QUEUE.to_owned()]));

    // Playing on and starting again leaves nothing behind.
    start(&queues, &db, OTHER, &[3]).await.unwrap();
    queues.save(&db).await;
    let second = list_hash(&[3]);
    assert_eq!((stored(&conn, 1), uses(&conn, 1)), (vec![second.clone()], vec![second]));
    assert_eq!((queue_ids(&conn, 0), queue_ids(&conn, 1)), (vec![], vec![OTHER.to_owned()]));
    assert!(row(&conn, QUEUE).is_none());
}

#[tokio::test]
async fn shared_list_survives_either_deletion() {
    for first in [QUEUE, OTHER] {
        let (_temp, db, conn) = setup();
        let (queues, shared) = adjusted_and_saved(&db).await;
        // Kept queues A and B play the same list, and C is active.
        start(&queues, &db, OTHER, &[1, 2]).await.unwrap();
        adjust(&queues);
        start(&queues, &db, THIRD, &[3]).await.unwrap();
        queues.save(&db).await;
        assert_eq!(queue_ids(&conn, 0), [QUEUE, OTHER]);
        let kept = list_hash(&[3]);
        assert_eq!(stored(&conn, 1), sorted(vec![shared.clone(), kept.clone()]));

        let second = if first == QUEUE { OTHER } else { QUEUE };
        queues.delete(&db, 1, id(first)).await.unwrap();
        assert_eq!(stored(&conn, 1), sorted(vec![shared.clone(), kept.clone()]));
        queues.delete(&db, 1, id(second)).await.unwrap();
        assert_eq!(stored(&conn, 1), [kept]);
    }
}

#[tokio::test]
async fn restore_unsaved_parked_queue() {
    let (_temp, db, conn) = setup();
    // Never saved, and adjusted: it holds edits the database lacks.
    let (account, first) = playing(QUEUE, &[1, 2, 3]);
    let queues = holding([(1, account)]);
    queues.lock().get_mut(&1).unwrap().state.apply(&Edit::Next).unwrap();
    adjust(&queues);
    let edited = serde_json::to_value(&queues.lock()[&1].state).unwrap();
    queues.lock().get_mut(&1).unwrap().version = 3;
    start(&queues, &db, OTHER, &[9]).await.unwrap();
    assert!(row(&conn, QUEUE).is_none());

    let restored = queues.restore(&db, 1, id(QUEUE), || None).await.unwrap();
    assert_eq!(restored, None);
    {
        let accounts = queues.lock();
        let account = &accounts[&1];
        assert_eq!((account.queue_id.as_str(), account.version), (QUEUE, 3));
        assert_eq!(serde_json::to_value(&account.state).unwrap(), edited);
        // The queue it replaced was untouched, so it went rather than being kept.
        assert!(account.parked.is_empty() && account.lists.is_empty() && !account.changed);
    }
    assert_eq!((queue_ids(&conn, 0), queue_ids(&conn, 1)), (vec![], vec![QUEUE.to_owned()]));
    assert_eq!((stored(&conn, 1), uses(&conn, 1)), (vec![first.clone()], vec![first]));
}

#[tokio::test]
async fn a_start_keeps_repeat_and_a_restore_keeps_its_own() {
    let (_temp, db, _conn) = setup();
    let (queues, _) = adjusted_and_saved(&db).await;
    let set =
        |mode| queues.lock().get_mut(&1).unwrap().state.apply(&Edit::Repeat { mode }).unwrap();
    set(Repeat::All);
    start(&queues, &db, OTHER, &[3]).await.unwrap();
    assert_eq!(queues.lock()[&1].state.repeat, Repeat::All);

    set(Repeat::Off);
    queues.restore(&db, 1, id(QUEUE), || None).await.unwrap();
    assert_eq!(queues.lock()[&1].state.repeat, Repeat::All);
}

#[tokio::test]
async fn restore_makes_a_kept_queue_active_and_clears_last_active() {
    let (_temp, db, conn) = setup();
    let (queues, first) = adjusted_and_saved(&db).await;
    start(&queues, &db, OTHER, &[3]).await.unwrap();
    queues.save(&db).await;
    let (active, last_active, _) = row(&conn, QUEUE).unwrap();
    assert_eq!(active, 0);
    assert!(last_active.is_some());

    let wrong = id(THIRD);
    assert!(matches!(queues.restore(&db, 1, wrong, || None).await, Err(KeepError::NotFound)));
    assert!(matches!(queues.restore(&db, 1, id(OTHER), || None).await, Err(KeepError::Active)));
    queues.restore(&db, 1, id(QUEUE), || None).await.unwrap();
    assert_eq!(row(&conn, QUEUE), Some((1, None, None)));
    assert_eq!(queue_ids(&conn, 1), [QUEUE]);
    // The queue it replaced was untouched, and its list went with it.
    assert!(row(&conn, OTHER).is_none());
    assert_eq!(stored(&conn, 1), [first]);
    assert_eq!(queues.lock()[&1].queue_id.as_str(), QUEUE);
}

#[tokio::test]
async fn restore_races_expiry() {
    // Each is one job on the writer, so the two orders are the only ones there are.
    for expiry_first in [true, false] {
        let (_temp, db, conn) = setup();
        let (queues, first) = adjusted_and_saved(&db).await;
        start(&queues, &db, OTHER, &[3]).await.unwrap();
        queues.save(&db).await;
        let parked = crate::db::now_ms() - crate::db::retention::WINDOW_MS - 1;
        conn.execute("UPDATE queues SET last_active = ?1 WHERE queue_id = ?2", (parked, QUEUE))
            .unwrap();
        let sweep = || crate::db::retention::sweep(&db, crate::db::now_ms());

        if expiry_first {
            sweep().await;
            let restored = queues.restore(&db, 1, id(QUEUE), || None).await;
            assert!(matches!(restored, Err(KeepError::NotFound)));
            assert_eq!(queues.lock()[&1].queue_id.as_str(), OTHER);
            assert!(row(&conn, QUEUE).is_none() && !stored(&conn, 1).contains(&first));
        } else {
            queues.restore(&db, 1, id(QUEUE), || None).await.unwrap();
            sweep().await;
            assert_eq!(row(&conn, QUEUE), Some((1, None, None)));
            assert_eq!(stored(&conn, 1), [first]);
        }
    }
}

#[tokio::test]
async fn restore_save_failure_restores_memory() {
    let (_temp, db, conn) = setup();
    let (queues, _) = adjusted_and_saved(&db).await;
    start(&queues, &db, OTHER, &[3]).await.unwrap();
    adjust(&queues);
    queues.save(&db).await;
    // A is kept in the database, B is parked in memory only, and C is active.
    start(&queues, &db, THIRD, &[4]).await.unwrap();
    conn.execute_batch(
        "CREATE TRIGGER refuse BEFORE INSERT ON queue_list_uses \
         BEGIN SELECT RAISE(ABORT, 'refused'); END",
    )
    .unwrap();
    let state_of = |queues: &Queues| {
        let accounts = queues.lock();
        let account = &accounts[&1];
        let parked: Vec<String> =
            account.parked.iter().map(|queue| queue.queue_id.as_str().to_owned()).collect();
        (
            account.queue_id.as_str().to_owned(),
            parked,
            serde_json::to_value(&account.state).unwrap(),
        )
    };
    let before = state_of(&queues);
    assert_eq!(before.1, [OTHER]);

    for target in [QUEUE, OTHER] {
        let restored = queues.restore(&db, 1, id(target), || at("kc.0", 5)).await;
        assert!(matches!(restored, Err(KeepError::Db(_))), "{target}");
        assert_eq!(state_of(&queues), before, "{target}");
    }
    assert_eq!(queue_ids(&conn, 0), [QUEUE]);

    // The account is still unsaved, so once the database takes it the next save writes it.
    conn.execute_batch("DROP TRIGGER refuse").unwrap();
    queues.save(&db).await;
    assert_eq!(
        (queue_ids(&conn, 1), queue_ids(&conn, 0)),
        (vec![THIRD.to_owned()], vec![QUEUE.to_owned(), OTHER.to_owned()])
    );

    // And the one that was parked in memory restores.
    queues.restore(&db, 1, id(OTHER), || None).await.unwrap();
    assert_eq!(queues.lock()[&1].queue_id.as_str(), OTHER);
    assert_eq!(queue_ids(&conn, 1), [OTHER]);
}

#[tokio::test]
async fn add_then_start_before_save_records_uses() {
    let (_temp, db, conn) = setup();
    // Never saved before: the queue, its add and the start all come in one second.
    let (account, first) = playing(QUEUE, &[1, 2]);
    let queues = holding([(1, account)]);
    let added = add_list(&queues, "b", &[7]);
    start(&queues, &db, OTHER, &[5]).await.unwrap();
    queues.save(&db).await;

    let second = list_hash(&[5]);
    assert_eq!(uses_of(&conn, QUEUE), sorted(vec![first.clone(), added.clone()]));
    assert_eq!(uses_of(&conn, OTHER), std::slice::from_ref(&second));
    assert_eq!(stored(&conn, 1), sorted(vec![first, added, second]));
}

#[tokio::test]
async fn add_during_save_then_start_keeps_held_list() {
    let (_temp, db, conn) = setup();
    let (account, first) = playing(QUEUE, &[1, 2]);
    let queues = holding([(1, account)]);
    // While the save's write is under way, a list is added and a start parks the queue.
    let saving = queues.clone();
    let saved = db
        .write(move |tx| {
            let saved = saving.write_changed(tx);
            add_list(&saving, "b", &[7]);
            saving.start_in(tx, 1, fresh(new_queue(OTHER, &[5])).unwrap(), || None).unwrap();
            Ok(saved)
        })
        .await
        .unwrap();
    queues.committed(saved);
    let added = list_hash(&[7]);
    {
        let accounts = queues.lock();
        let held: Vec<&String> = accounts[&1].lists.keys().collect();
        assert_eq!(
            sorted(held.into_iter().cloned().collect()),
            sorted(vec![added.clone(), list_hash(&[5])])
        );
    }

    queues.save(&db).await;
    assert_eq!(uses_of(&conn, QUEUE), sorted(vec![first.clone(), added.clone()]));
    assert_eq!(stored(&conn, 1), sorted(vec![first, added, list_hash(&[5])]));
    assert!(queues.lock()[&1].lists.is_empty());
}

#[tokio::test]
async fn start_then_restore_within_second_writes_once() {
    let (_temp, db, conn) = setup();
    let (account, first) = playing(QUEUE, &[1, 2]);
    let queues = holding([(1, account)]);
    adjust(&queues);
    start(&queues, &db, OTHER, &[3]).await.unwrap();
    queues.restore(&db, 1, id(QUEUE), || None).await.unwrap();
    assert!(!queues.lock()[&1].changed);

    // The restore saved the account, so the background save has nothing to write.
    conn.execute("UPDATE queues SET version = 9", []).unwrap();
    queues.save(&db).await;
    assert_eq!(row(&conn, QUEUE), Some((1, None, None)));
    let version: u32 = conn.query_row("SELECT version FROM queues", [], |r| r.get(0)).unwrap();
    assert_eq!((version, queue_ids(&conn, 0), stored(&conn, 1)), (9, vec![], vec![first]));
}

#[tokio::test]
async fn unadjusted_parked_queue_is_404() {
    let (_temp, db, conn) = setup();
    let (account, _) = playing(QUEUE, &[1, 2]);
    let queues = holding([(1, account)]);
    start(&queues, &db, OTHER, &[3]).await.unwrap();

    let found = |result: Result<(), KeepError>| matches!(result, Err(KeepError::NotFound));
    assert!(found(queues.restore(&db, 1, id(QUEUE), || None).await.map(drop)));
    assert!(found(queues.delete(&db, 1, id(QUEUE)).await));
    // Both left it for the save to delete, and neither changed what was playing.
    assert_eq!(queues.lock()[&1].queue_id.as_str(), OTHER);
    assert_eq!(queues.lock()[&1].parked.len(), 1);
    queues.save(&db).await;
    assert!(row(&conn, QUEUE).is_none());
}

#[tokio::test]
async fn delete_unsaved_parked_queue() {
    let (_temp, db, conn) = setup();
    let (account, _) = playing(QUEUE, &[1, 2]);
    let queues = holding([(1, account)]);
    adjust(&queues);
    start(&queues, &db, OTHER, &[3]).await.unwrap();
    assert!(matches!(queues.delete(&db, 1, id(OTHER)).await, Err(KeepError::Active)));

    queues.delete(&db, 1, id(QUEUE)).await.unwrap();
    assert!(queues.lock()[&1].parked.is_empty());
    queues.save(&db).await;
    let kept = list_hash(&[3]);
    assert_eq!((queue_ids(&conn, 0), stored(&conn, 1)), (vec![], vec![kept]));
    assert!(queues.lock()[&1].lists.is_empty());
    // Gone, so there is nothing to find.
    assert!(matches!(queues.delete(&db, 1, id(QUEUE)).await, Err(KeepError::NotFound)));
}

#[tokio::test]
async fn failed_delete_puts_the_queue_back_on_the_writer() {
    let (_temp, db, conn) = setup();
    let (account, _) = playing(QUEUE, &[1, 2]);
    let queues = holding([(1, account)]);
    queues.save(&db).await;
    adjust(&queues);
    start(&queues, &db, OTHER, &[3]).await.unwrap();
    conn.execute_batch(
        "CREATE TRIGGER refuse BEFORE DELETE ON queues \
         BEGIN SELECT RAISE(ABORT, 'refused'); END",
    )
    .unwrap();

    // Inside the writer, before any other save could run, the queue is back and marked.
    let inside = queues.clone();
    let back = db
        .try_write(move |tx| {
            let slot = Default::default();
            let result = inside.delete_in(tx, 1, &id(QUEUE), &slot);
            let accounts = inside.lock();
            let account = &accounts[&1];
            let back = account.parked.iter().any(|q| q.queue_id.as_str() == QUEUE);
            Ok::<_, KeepError>((result.is_err(), back, account.changed))
        })
        .await
        .unwrap();
    assert_eq!(back, (true, true, true));
}

#[tokio::test]
async fn two_starts_in_one_second_keep_one_active_row() {
    let (_temp, db, conn) = setup();
    let (queues, _) = adjusted_and_saved(&db).await;
    start(&queues, &db, OTHER, &[3]).await.unwrap();
    adjust(&queues);
    start(&queues, &db, THIRD, &[4]).await.unwrap();
    queues.save(&db).await;

    assert_eq!(
        (queue_ids(&conn, 1), queue_ids(&conn, 0)),
        (vec![THIRD.to_owned()], vec![QUEUE.to_owned(), OTHER.to_owned()])
    );
    assert_eq!(stored(&conn, 1).len(), 3);
}

#[tokio::test]
async fn restore_browse_start_keeps_position() {
    let (_temp, db, conn) = setup();
    let (queues, _) = adjusted_and_saved(&db).await;
    // Parked while the player was 1.5 s into its current item.
    queues.start(&db, 1, new_queue(OTHER, &[3]), || at("a.0", 1500)).await.unwrap();
    queues.save(&db).await;
    let kept = row(&conn, QUEUE).unwrap().2;
    assert_eq!(kept.as_deref(), Some(r#"{"item":"a.0","positionMs":1500}"#));

    // Restored, and the player names an item of the other queue until its first report
    // replaces it. Starting again does not lose the position.
    let position = queues.restore(&db, 1, id(QUEUE), || None).await.unwrap();
    assert_eq!(position, at("a.0", 1500));
    queues.start(&db, 1, new_queue(THIRD, &[4]), || at("kb.0", 10)).await.unwrap();
    queues.save(&db).await;
    assert_eq!(row(&conn, QUEUE).unwrap().2, kept);
    // And once restored again, a start after a restart of the server still keeps it.
    queues.restore(&db, 1, id(QUEUE), || None).await.unwrap();
    let restarted = Arc::new(Queues::load(&db).await.unwrap());
    restarted
        .start(&db, 1, new_queue("44444444-4444-4444-8444-444444444444", &[5]), || at("zzz.9", 1))
        .await
        .unwrap();
    restarted.save(&db).await;
    assert_eq!(row(&conn, QUEUE).unwrap().2, kept);
}

#[tokio::test]
async fn a_start_naming_any_of_the_accounts_queues_changes_nothing() {
    let (_temp, db, conn) = setup();
    let (queues, _) = adjusted_and_saved(&db).await;
    start(&queues, &db, OTHER, &[3]).await.unwrap();
    queues.save(&db).await;
    adjust(&queues);
    // QUEUE is kept, OTHER is active and THIRD is parked, unsaved.
    start(&queues, &db, THIRD, &[4]).await.unwrap();
    let before = queues.lock()[&1].parked.len();

    for named in [QUEUE, OTHER, THIRD] {
        let started = start(&queues, &db, named, &[9]).await;
        assert!(matches!(started, Err(KeepError::Exists)), "{named}");
    }
    {
        let accounts = queues.lock();
        assert_eq!((accounts[&1].queue_id.as_str(), accounts[&1].parked.len()), (THIRD, before));
    }
    queues.save(&db).await;
    assert_eq!(queue_ids(&conn, 0), [QUEUE, OTHER]);
    assert!(!stored(&conn, 1).contains(&list_hash(&[9])));
}
