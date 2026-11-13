//! `libraryChanged` and `accountChanged`: sent after a change commits, `libraryChanged` only to
//! accounts that reach the library, at most once a second for each library or account, and never
//! leaving the last change of a burst unhinted.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::Message;
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio::time::{Instant, timeout};

use super::super::hints::{Throttle, WINDOW, start};
use super::super::{Hub, Presence};
use crate::db::Database;
use crate::db::feed::{self, Entity, Kind, Op};
use crate::db::testing::{User, access, library, open};

/// A database with libraries 1 and 2, the owner (account 1), account 2 who reaches library 1,
/// and account 3 who reaches none, and a hub the hints go through.
async fn setup() -> (tempfile::TempDir, Arc<Database>, Arc<Hub>) {
    let (temp, db) = open();
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        library(tx, 2, "Other", "/other")?;
        User::new(1, "owner").insert(tx)?;
        User::new(2, "user").insert(tx)?;
        User::new(3, "user").insert(tx)?;
        access(tx, 2, 1)
    })
    .await
    .unwrap();
    let hub = Arc::new(Hub::default());
    // Dropped with the runtime at the end of the test.
    drop(start(db.clone(), hub.clone()));
    (temp, db, hub)
}

/// Records an upsert of each of `tracks` in `library`'s feed in one transaction, and returns the
/// feed's position after it.
async fn change(db: &Database, library: i64, tracks: &[i64]) -> i64 {
    let tracks = tracks.to_vec();
    db.write(move |tx| {
        for track in tracks {
            feed::record(tx, library, Entity::Track, track, Op::Upsert, 0)?;
        }
        tx.query_row("SELECT max(seq) FROM library_changes", [], |row| row.get(0))
    })
    .await
    .unwrap()
}

/// The next message `presence` is sent, as JSON. Panics if none comes within 5 seconds.
async fn heard(presence: &mut Presence) -> Value {
    let message = timeout(Duration::from_secs(5), presence.next()).await.expect("a message");
    text(message)
}

/// [`heard`] for a test with time paused, where a timeout would elapse while the database's
/// threads work. It polls without a timer, so the clock stays where the test put it, and gives
/// up after 5 real seconds.
async fn heard_paused(presence: &mut Presence) -> Value {
    let give_up = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < give_up {
        if let Ok(message) = timeout(Duration::ZERO, presence.next()).await {
            return text(message);
        }
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("no message");
}

/// Waits for the feed to move past what `feed` has seen, then gives the hint task 200 real
/// milliseconds to read the change, without moving the paused clock. The task takes the time of
/// a change when it wakes, so the change is held for the window only once the head has moved.
async fn settle(feed: &mut watch::Receiver<i64>) {
    let give_up = std::time::Instant::now() + Duration::from_secs(5);
    while !feed.has_changed().unwrap() {
        assert!(std::time::Instant::now() < give_up, "the feed did not move");
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(1));
    }
    feed.borrow_and_update();
    let until = std::time::Instant::now() + Duration::from_millis(200);
    while std::time::Instant::now() < until {
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// `message` as JSON.
fn text(message: Option<Message>) -> Value {
    let Some(Message::Text(text)) = message else { panic!("no text message: {message:?}") };
    serde_json::from_str(text.as_str()).unwrap()
}

/// Whether `presence` has a message waiting.
async fn hears_nothing(presence: &mut Presence) -> bool {
    timeout(Duration::ZERO, presence.next()).await.is_err()
}

#[tokio::test]
async fn a_scan_batch_sends_one_library_changed_to_accounts_with_access() {
    let (_temp, db, hub) = setup().await;
    let (mut owner, mut user) = (hub.connect(1, 1), hub.connect(2, 2));
    let mut tab = hub.connect(2, 2);

    let seq = change(&db, 1, &[1, 2, 3]).await;

    let hint = json!({ "type": "libraryChanged", "libraryId": "1", "seq": seq });
    assert_eq!(heard(&mut owner).await, hint);
    assert_eq!(heard(&mut user).await, hint);
    assert_eq!(heard(&mut tab).await, hint, "every connection of the account hears it");
    assert!(hears_nothing(&mut owner).await, "one hint for the batch");
}

#[tokio::test]
async fn an_account_without_access_hears_nothing() {
    let (_temp, db, hub) = setup().await;
    let (mut owner, mut user, mut stranger) =
        (hub.connect(1, 1), hub.connect(2, 2), hub.connect(3, 3));

    let seq = change(&db, 2, &[1]).await;

    let hint = json!({ "type": "libraryChanged", "libraryId": "2", "seq": seq });
    assert_eq!(heard(&mut owner).await, hint);
    assert!(hears_nothing(&mut user).await, "account 2 reaches library 1 only");
    assert!(hears_nothing(&mut stranger).await);
}

#[tokio::test]
async fn a_suspended_account_hears_nothing() {
    let (_temp, db, hub) = setup().await;
    let (mut owner, mut user) = (hub.connect(1, 1), hub.connect(2, 2));
    db.write(|tx| tx.execute("UPDATE users SET status = 'suspended' WHERE id = 2", []).map(drop))
        .await
        .unwrap();

    change(&db, 1, &[1]).await;

    assert_eq!(heard(&mut owner).await["libraryId"], json!("1"));
    assert!(hears_nothing(&mut user).await, "account 2 has access but is suspended");
}

#[tokio::test]
async fn each_hint_carries_its_own_librarys_seq() {
    let (_temp, db, hub) = setup().await;
    let mut owner = hub.connect(1, 1);

    let (first, second) = db
        .write(|tx| {
            feed::record(tx, 1, Entity::Track, 1, Op::Upsert, 0)?;
            let first: i64 =
                tx.query_row("SELECT max(seq) FROM library_changes", [], |r| r.get(0))?;
            feed::record(tx, 2, Entity::Track, 2, Op::Upsert, 0)?;
            let second: i64 =
                tx.query_row("SELECT max(seq) FROM library_changes", [], |r| r.get(0))?;
            Ok((first, second))
        })
        .await
        .unwrap();

    let mut heard_both = [heard(&mut owner).await, heard(&mut owner).await];
    heard_both.sort_by_key(|hint| hint["libraryId"].as_str().unwrap().to_owned());
    assert_eq!(heard_both[0], json!({ "type": "libraryChanged", "libraryId": "1", "seq": first }));
    assert_eq!(heard_both[1], json!({ "type": "libraryChanged", "libraryId": "2", "seq": second }));
}

#[tokio::test(start_paused = true)]
async fn a_library_is_hinted_again_only_when_it_changes() {
    let (_temp, db, hub) = setup().await;
    let mut owner = hub.connect(1, 1);
    let mut feed = db.follow_feed();

    change(&db, 1, &[1]).await;
    assert_eq!(heard_paused(&mut owner).await["libraryId"], json!("1"));
    change(&db, 2, &[2]).await;
    assert_eq!(heard_paused(&mut owner).await["libraryId"], json!("2"));
    feed.borrow_and_update();
    let again = change(&db, 1, &[3]).await;
    settle(&mut feed).await;
    tokio::time::advance(WINDOW).await;

    let hint = heard_paused(&mut owner).await;
    assert_eq!((&hint["libraryId"], &hint["seq"]), (&json!("1"), &json!(again)));
    tokio::time::sleep(2 * WINDOW).await;
    assert!(hears_nothing(&mut owner).await, "library 2 did not change");
}

#[tokio::test(start_paused = true)]
async fn the_last_change_of_a_burst_is_always_hinted() {
    let (_temp, db, hub) = setup().await;
    let mut owner = hub.connect(1, 1);

    let first = change(&db, 1, &[1]).await;
    assert_eq!(heard_paused(&mut owner).await["seq"], json!(first));
    // Inside the first hint's window, so held until it ends.
    let mut feed = db.follow_feed();
    let last = change(&db, 1, &[2, 3]).await;
    settle(&mut feed).await;
    assert!(hears_nothing(&mut owner).await, "one hint a second");

    tokio::time::advance(WINDOW).await;

    assert_eq!(heard_paused(&mut owner).await["seq"], json!(last));
    tokio::time::sleep(2 * WINDOW).await;
    assert!(hears_nothing(&mut owner).await);
}

#[test]
fn a_burst_of_changes_sends_at_most_one_hint_per_second_with_the_latest_seq() {
    let start = Instant::now();
    let at = |millis| start + Duration::from_millis(millis);
    let mut throttle = Throttle::default();

    assert_eq!(throttle.moved(at(0), 1, 10), Some(10), "the first change is hinted at once");
    for (millis, seq) in [(100, 11), (500, 14), (900, 13)] {
        assert_eq!(throttle.moved(at(millis), 1, seq), None);
    }
    assert_eq!(throttle.next_deadline(), Some(at(1000)));
    assert_eq!(throttle.due(at(999)), vec![]);
    assert_eq!(throttle.due(at(1000)), vec![(1, 14)], "the latest seq, once");
    assert_eq!(throttle.next_deadline(), None);
    // The hint at 1000 opened a new window.
    assert_eq!(throttle.moved(at(1500), 1, 15), None);
    assert_eq!(
        throttle.moved(at(2000), 1, 16),
        Some(16),
        "a change as the window ends is hinted at once, superseding the held one"
    );
    // Libraries are throttled apart.
    assert_eq!(throttle.moved(at(2100), 2, 3), Some(3));
}

/// Records a change to `user`'s playlist 7 in library 1 and returns the account feed's position.
async fn account_change(db: &Database, user: i64) -> i64 {
    db.write(move |tx| {
        feed::record_account(tx, user, 1, Kind::Playlist, 7, 0, Op::Upsert)?;
        tx.query_row("SELECT max(seq) FROM account_changes", [], |row| row.get(0))
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn account_changed_reaches_only_the_account() {
    let (_temp, db, hub) = setup().await;
    let (mut owner, mut user, mut stranger) =
        (hub.connect(1, 1), hub.connect(2, 2), hub.connect(3, 3));
    let mut tab = hub.connect(2, 2);

    let seq = account_change(&db, 2).await;

    let hint = json!({ "type": "accountChanged", "seq": seq });
    assert_eq!(heard(&mut user).await, hint);
    assert_eq!(heard(&mut tab).await, hint, "every connection of the account hears it");
    // Account 1 reaches library 1 as the owner, and 3 reaches none: neither hears of account 2.
    assert!(hears_nothing(&mut owner).await);
    assert!(hears_nothing(&mut stranger).await);
    assert!(hears_nothing(&mut user).await, "one hint for the change");
}

#[tokio::test(start_paused = true)]
async fn the_last_account_change_of_a_burst_is_always_hinted() {
    let (_temp, db, hub) = setup().await;
    let mut user = hub.connect(2, 2);

    let first = account_change(&db, 2).await;
    assert_eq!(heard_paused(&mut user).await["seq"], json!(first));
    let mut feed = db.follow_account_feed();
    account_change(&db, 2).await;
    let last = account_change(&db, 2).await;
    settle(&mut feed).await;
    assert!(hears_nothing(&mut user).await, "one hint a second");

    tokio::time::advance(WINDOW).await;

    assert_eq!(heard_paused(&mut user).await["seq"], json!(last));
}
