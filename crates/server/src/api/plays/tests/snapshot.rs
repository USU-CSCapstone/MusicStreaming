//! Each play's snapshot of its track, which keeps its history readable once the track is purged
//! (`requirements/analytics.md` §8, `requirements/scanning.md` §8.1).

use std::slice;

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::super::testing::catalog;
use super::{add_second_user, as_second_user, post, report, report_as_owner};
use crate::db::Database;

/// Each play's `[user, title, artists, album]`, in user and ID order.
async fn snapshots(db: &Database) -> Vec<Value> {
    let rows = db
        .read(|conn| {
            conn.prepare(
                "SELECT json_array(user_id, title, json(artists), album) FROM plays \
                 ORDER BY user_id, id",
            )?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        .unwrap();
    rows.iter().map(|row| serde_json::from_str(row).unwrap()).collect()
}

#[tokio::test]
async fn a_play_keeps_its_tracks_title_artists_and_album() {
    let (_temp, db, app) = catalog::app().await;
    let mut untitled = report("1006", 0, Value::Null);
    untitled["playId"] = json!("0e3f4b1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b");
    let reports = json!([report("1004", 0, Value::Null), untitled]);
    assert_eq!(report_as_owner(&app, reports).await, StatusCode::NO_CONTENT);
    assert_eq!(
        snapshots(&db).await,
        [
            json!([1, "untitled", [], null]),
            json!([1, "Bonus", ["The Beatles", "Radiohead"], "Abbey Road"]),
        ],
        "artists in credit order as the catalog names them; the unknown artist and album have none"
    );
}

#[tokio::test]
async fn a_snapshot_follows_a_retag_and_outlives_its_track() {
    let (_temp, db, app) = catalog::app().await;
    assert_eq!(
        report_as_owner(&app, json!([report("1001", 0, Value::Null)])).await,
        StatusCode::NO_CONTENT
    );
    db.write(|tx| {
        tx.execute_batch(
            "UPDATE tracks SET title = 'Come Together (Remastered)' WHERE id = 1001;
             UPDATE artists SET name = 'Beatles' WHERE id = 10;
             UPDATE albums SET title = 'Abbey Road (Remastered)' WHERE id = 101;",
        )
    })
    .await
    .unwrap();
    assert_eq!(
        report_as_owner(&app, json!([report("1001", 10_000, Value::Null)])).await,
        StatusCode::NO_CONTENT
    );
    let retagged = json!([1, "Come Together (Remastered)", ["Beatles"], "Abbey Road (Remastered)"]);
    assert_eq!(
        snapshots(&db).await,
        slice::from_ref(&retagged),
        "a later report takes the new tags"
    );

    // A purge: the track goes, and its credits, tags and lyrics with it.
    db.write(|tx| tx.execute("DELETE FROM tracks WHERE id = 1001", [])).await.unwrap();
    assert_eq!(snapshots(&db).await, slice::from_ref(&retagged), "the play outlives its track");
    assert_eq!(
        report_as_owner(&app, json!([report("1001", 20_000, json!("finished"))])).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(snapshots(&db).await, [retagged], "a late report changes nothing of it");
}

#[tokio::test]
async fn two_users_snapshots_are_each_their_own() {
    let (temp, db, app) = catalog::app().await;
    add_second_user(&temp);
    assert_eq!(
        report_as_owner(&app, json!([report("1001", 0, Value::Null)])).await,
        StatusCode::NO_CONTENT
    );
    let theirs = json!([report("1004", 0, Value::Null)]);
    assert_eq!(post(&app, as_second_user(), theirs).await, StatusCode::NO_CONTENT);
    assert_eq!(
        snapshots(&db).await,
        [
            json!([1, "Come Together", ["The Beatles"], "Abbey Road"]),
            json!([2, "Bonus", ["The Beatles", "Radiohead"], "Abbey Road"]),
        ],
        "the same playId of another track is another user's play, with its own snapshot"
    );
}

#[tokio::test]
async fn a_play_id_reported_with_another_track_changes_nothing() {
    let (_temp, db, app) = catalog::app().await;
    assert_eq!(
        report_as_owner(&app, json!([report("1001", 0, Value::Null)])).await,
        StatusCode::NO_CONTENT
    );
    let mut elsewhere = report("2001", 10_000, json!("finished"));
    elsewhere["libraryId"] = json!("2");
    let reports = json!([report("1004", 10_000, json!("skipped")), elsewhere]);
    assert_eq!(report_as_owner(&app, reports).await, StatusCode::NO_CONTENT);
    assert_eq!(
        snapshots(&db).await,
        [json!([1, "Come Together", ["The Beatles"], "Abbey Road"])],
        "neither another track in its library nor one in another library"
    );
    let heard = db
        .read(|conn| {
            conn.query_row("SELECT listen_time_ms, ended FROM plays", [], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
            })
        })
        .await
        .unwrap();
    assert_eq!(heard, (0, None), "nor does its listen time or end");
}

#[tokio::test]
async fn a_snapshots_artists_must_be_an_array() {
    let (_temp, db, _app) = catalog::app().await;
    let insert = db.write(|tx| {
        tx.execute(
            "INSERT INTO plays (id, user_id, library_id, track_id, title, artists, started_at, \
                                listen_time_ms, context, origin, updated_at) \
             VALUES ('p', 1, 1, 1001, 'A', '{}', 0, 0, '{}', 'context', 0)",
            [],
        )
    });
    assert!(insert.await.unwrap_err().to_string().contains("CHECK constraint failed"));
}
