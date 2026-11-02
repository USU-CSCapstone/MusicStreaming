//! The queue store and its background save. Tests drive `save` directly, never the timer.

use std::collections::HashMap;
use std::sync::Arc;

use jewelcase_core::{QueueState, Source};
use rusqlite::Connection;
use serde_json::json;

use super::hash::{list_bytes, list_hash};
use super::store::{Account, Queues};
use crate::api::Uuid;
use crate::api::testing::{add_user, conn};
use crate::db::Database;
use crate::db::testing::open;

const QUEUE: &str = "11111111-1111-4111-8111-111111111111";
const OTHER: &str = "22222222-2222-4222-8222-222222222222";

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
