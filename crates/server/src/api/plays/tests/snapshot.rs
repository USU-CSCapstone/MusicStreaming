//! Each play's snapshot of its track, which keeps its history readable and its statistics whole
//! once the track is purged (`requirements/analytics.md` §8, `requirements/scanning.md` §8.1).

use std::slice;
use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::super::testing::catalog;
use super::{report, report_as_owner};
use crate::db::Database;

/// The shared catalog, with the keys the scanner would give its artists and albums, and the
/// untitled track credited to Radiohead before the unknown artist. The unknown artist's key is
/// empty, and so is the unknown album's title key; its artists' key is that of the track's
/// credits, as the scanner keys an album with no album-artist tag.
pub(super) async fn app() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = catalog::app().await;
    db.write(|tx| {
        tx.execute_batch(
            "UPDATE track_artists SET position = 1 WHERE track_id = 1006;
             INSERT INTO track_artists (library_id, track_id, position, artist_id)
                 VALUES (1, 1006, 0, 11);
             UPDATE artists SET name_key = 'the beatles' WHERE id = 10;
             UPDATE artists SET name_key = 'radiohead' WHERE id = 11;
             UPDATE artists SET name_key = '' WHERE id = 13;
             UPDATE artists SET name_key = 'elsewhere' WHERE id = 20;
             UPDATE albums SET title_key = 'abbey road', artists_key = 'the beatles' WHERE id = 101;
             UPDATE albums SET title_key = 'kid a', artists_key = 'radiohead' WHERE id = 102;
             UPDATE albums SET title_key = '', artists_key = ';radiohead' WHERE id = 104;
             UPDATE albums SET title_key = 'elsewhere', artists_key = 'elsewhere' WHERE id = 201;",
        )
    })
    .await
    .unwrap();
    (temp, db, app)
}

/// Each play's user and then `columns`, as JSON arrays, in user and play ID order.
pub(super) async fn snapshots(db: &Database, columns: &'static str) -> Vec<Value> {
    let rows = db
        .read(move |conn| {
            conn.prepare(&format!(
                "SELECT json_array(user_id, {columns}) FROM plays ORDER BY user_id, id"
            ))?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()
        })
        .await
        .unwrap();
    rows.iter().map(|row| serde_json::from_str(row).unwrap()).collect()
}

/// The title, and the artists and album with the keys the catalog matches them by.
const CREDITS: &str =
    "title, json(artists), json(artist_keys), album, album_title_key, album_artists_key";

#[tokio::test]
async fn a_play_keeps_its_tracks_names_and_keys_in_credit_order() {
    let (_temp, db, app) = app().await;
    let mut idioteque = report("1005", 0, Value::Null);
    idioteque["playId"] = json!("1e3f4b1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b");
    let mut come_together = report("1001", 0, Value::Null);
    come_together["playId"] = json!("2e3f4b1a-1c2d-4e5f-8a9b-0c1d2e3f4a5b");
    let reports = json!([report("1004", 0, Value::Null), idioteque, come_together]);
    assert_eq!(report_as_owner(&app, reports).await, StatusCode::NO_CONTENT);
    let columns = "title, json(artists), json(artist_keys), album, album_title_key, \
                   album_artists_key, json(genre_keys)";
    assert_eq!(
        snapshots(&db, columns).await,
        [
            json!([
                1,
                "Idioteque",
                ["Radiohead", "The Beatles"],
                ["radiohead", "the beatles"],
                "Kid A",
                "kid a",
                "radiohead",
                []
            ]),
            json!([
                1,
                "Come Together",
                ["The Beatles"],
                ["the beatles"],
                "Abbey Road",
                "abbey road",
                "the beatles",
                ["rock"]
            ]),
            json!([
                1,
                "Bonus",
                ["The Beatles", "Radiohead"],
                ["the beatles", "radiohead"],
                "Abbey Road",
                "abbey road",
                "the beatles",
                []
            ]),
        ],
        "each track's own genres, and none from another track"
    );
}

#[tokio::test]
async fn an_untagged_play_keeps_the_unknown_artist_and_album_through_a_purge() {
    let (_temp, db, app) = app().await;
    let untagged = || json!([report("1006", 0, Value::Null)]);
    assert_eq!(report_as_owner(&app, untagged()).await, StatusCode::NO_CONTENT);
    let kept =
        json!([1, "untitled", ["Radiohead", null], ["radiohead", ""], null, "", ";radiohead"]);
    let message = "the unknown artist has a null name and an empty key; the unknown album a null \
                   title, an empty title key and its artists' key";
    assert_eq!(snapshots(&db, CREDITS).await, slice::from_ref(&kept), "{message}");

    db.write(|tx| tx.execute("DELETE FROM tracks WHERE id = 1006", [])).await.unwrap();
    assert_eq!(report_as_owner(&app, untagged()).await, StatusCode::NO_CONTENT);
    assert_eq!(snapshots(&db, CREDITS).await, [kept], "the keys outlive the track");
}

#[tokio::test]
async fn a_snapshot_follows_a_retag_and_outlives_its_track() {
    let (_temp, db, app) = app().await;
    let columns = "title, duration_us, release_date, json(genre_keys), json(artists), \
                   json(artist_keys), album, album_title_key, album_artists_key";
    assert_eq!(
        report_as_owner(&app, json!([report("1001", 0, Value::Null)])).await,
        StatusCode::NO_CONTENT
    );
    let first = json!([
        1,
        "Come Together",
        1_000_000,
        "1969",
        ["rock"],
        ["The Beatles"],
        ["the beatles"],
        "Abbey Road",
        "abbey road",
        "the beatles"
    ]);
    assert_eq!(snapshots(&db, columns).await, [first]);
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO artists (id, library_id, name, name_key, sort_key, added_at)
                 VALUES (14, 1, 'Beatles', 'beatles', x'00', 0);
             INSERT INTO albums (id, library_id, title, title_key, artists_key, sort_key,
                                 artist_sort_key, added_at)
                 VALUES (105, 1, 'Abbey Road (Remastered)', 'abbey road (remastered)',
                         'beatles', x'00', x'00', 0);
             UPDATE track_artists SET artist_id = 14 WHERE track_id = 1001;
             INSERT INTO track_tags (library_id, track_id, tag_id, tag_name)
                 VALUES (1, 1001, 31, 'pop');
             UPDATE tracks SET title = 'Come Together (Remastered)', album_id = 105,
                               duration_us = 2000000, release_date = '2019-09-27'
                 WHERE id = 1001;",
        )
    })
    .await
    .unwrap();
    assert_eq!(
        report_as_owner(&app, json!([report("1001", 10_000, Value::Null)])).await,
        StatusCode::NO_CONTENT
    );
    let retagged = json!([
        1,
        "Come Together (Remastered)",
        2_000_000,
        "2019-09-27",
        ["pop", "rock"],
        ["Beatles"],
        ["beatles"],
        "Abbey Road (Remastered)",
        "abbey road (remastered)",
        "beatles"
    ]);
    let message = "a later report takes the new tags, with the genres in key order";
    assert_eq!(snapshots(&db, columns).await, slice::from_ref(&retagged), "{message}");

    // A purge: the track goes, and its credits, tags and lyrics with it.
    db.write(|tx| tx.execute("DELETE FROM tracks WHERE id = 1001", [])).await.unwrap();
    assert_eq!(snapshots(&db, columns).await, slice::from_ref(&retagged), "the play outlives it");
    assert_eq!(
        report_as_owner(&app, json!([report("1001", 20_000, json!("finished"))])).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(snapshots(&db, columns).await, [retagged], "a late report changes nothing of it");
}

#[tokio::test]
async fn a_play_id_reported_with_another_track_changes_nothing() {
    let (_temp, db, app) = app().await;
    assert_eq!(
        report_as_owner(&app, json!([report("1001", 0, Value::Null)])).await,
        StatusCode::NO_CONTENT
    );
    let mut elsewhere = report("2001", 10_000, json!("finished"));
    elsewhere["libraryId"] = json!("2");
    let reports = json!([report("1004", 10_000, json!("skipped")), elsewhere]);
    assert_eq!(report_as_owner(&app, reports).await, StatusCode::NO_CONTENT);
    assert_eq!(
        snapshots(&db, "title, json(artist_keys), listen_time_ms, ended").await,
        [json!([1, "Come Together", ["the beatles"], 0, null])],
        "neither another track in its library nor one in another library, nor its listen time"
    );
}
