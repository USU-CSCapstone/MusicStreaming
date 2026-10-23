//! A user's plays are theirs alone: another user's reports never reach them, and they stay when
//! the library they were heard in is deleted or their access to it is revoked
//! (`requirements/analytics.md` §8).

use std::slice;

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::snapshot::{app, snapshots};
use super::{add_second_user, events, post, report, report_as_owner};
use crate::api::testing::conn;
use crate::db::testing::library;

#[tokio::test]
async fn deleting_a_library_keeps_its_plays_without_it() {
    let (_temp, db, app) = app().await;
    let mut elsewhere = report("2001", 0, Value::Null);
    elsewhere["libraryId"] = json!("2");
    assert_eq!(report_as_owner(&app, json!([elsewhere])).await, StatusCode::NO_CONTENT);
    assert_eq!(events(&db, "started").await, 1);

    db.write(|tx| tx.execute("DELETE FROM libraries WHERE id = 2", [])).await.unwrap();
    let kept = json!([1, null, "Elsewhere", ["elsewhere"], "elsewhere"]);
    let columns = "library_id, title, json(artist_keys), album_title_key";
    assert_eq!(snapshots(&db, columns).await, slice::from_ref(&kept), "it keeps its snapshot");
    assert_eq!(events(&db, "started").await, 0, "no hook delivers it for a library gone");

    // Library IDs can be used again, and a new library 2 takes none of the old one's plays.
    db.write(|tx| library(tx, 2, "Again", "/again")).await.unwrap();
    assert_eq!(snapshots(&db, columns).await, [kept]);
}

#[tokio::test]
async fn a_user_keeps_their_plays_after_losing_access() {
    let (temp, db, app) = app().await;
    add_second_user(&temp);
    let theirs = json!([report("1001", 0, Value::Null)]);
    assert_eq!(post(&app, "user-token", theirs).await, StatusCode::NO_CONTENT);
    let conn = conn(&temp);
    conn.execute("DELETE FROM library_access WHERE user_id = 2", []).unwrap();

    let later = json!([report("1001", 30_000, json!("finished"))]);
    assert_eq!(post(&app, "user-token", later).await, StatusCode::NO_CONTENT);
    let columns = "library_id, title, listen_time_ms, ended";
    let kept = json!([2, 1, "Come Together", 0, null]);
    let message = "the play stays theirs, and a report out of reach changes nothing of it";
    assert_eq!(snapshots(&db, columns).await, slice::from_ref(&kept), "{message}");

    let mine = json!([report("1004", 10_000, Value::Null)]);
    assert_eq!(report_as_owner(&app, mine).await, StatusCode::NO_CONTENT);
    assert_eq!(
        snapshots(&db, columns).await,
        [json!([1, 1, "Bonus", 10_000, null]), kept],
        "another user's play under the same ID is their own"
    );
}

#[tokio::test]
async fn two_users_snapshots_are_each_their_own() {
    let (temp, db, app) = app().await;
    add_second_user(&temp);
    assert_eq!(
        report_as_owner(&app, json!([report("1001", 0, Value::Null)])).await,
        StatusCode::NO_CONTENT
    );
    let theirs = json!([report("1004", 0, Value::Null)]);
    assert_eq!(post(&app, "user-token", theirs).await, StatusCode::NO_CONTENT);
    assert_eq!(
        snapshots(&db, "title, json(artist_keys)").await,
        [
            json!([1, "Come Together", ["the beatles"]]),
            json!([2, "Bonus", ["the beatles", "radiohead"]]),
        ],
        "the same playId of another track is another user's play, with its own snapshot"
    );
}
