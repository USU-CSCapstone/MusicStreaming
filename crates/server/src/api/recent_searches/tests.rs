use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use jewelcase_plugins::Permission::{LibraryRead, SearchActivity, Searched};
use serde_json::{Value, json};

use super::super::testing::{TOKEN, add_device, add_user, app_with_tracks, call, conn, json};
use super::{KEPT, WINDOW_MS};
use crate::db::testing::{Artist, Track, access, sweep};
use crate::db::{Database, now_ms};
use crate::plugins::personal::set_search_sharing;
use crate::plugins::testing::{caller, connect, enable, grant, install, manifest, plugins};

const URI: &str = "/api/v1/libraries/1/recent-searches";

/// Records `body` as the owner, and answers the status and the JSON body.
async fn record(app: &Router, body: Value) -> (StatusCode, Value) {
    call(app, TOKEN, "POST", URI, Some(body)).await
}

/// The queries of the recent searches the device with `token` lists in library 1, newest
/// first.
async fn listed(app: &Router, token: &str) -> Vec<String> {
    let (status, list) = call(app, token, "GET", URI, None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items = list["items"].as_array().unwrap();
    items.iter().map(|item| item["query"].as_str().unwrap().to_owned()).collect()
}

/// Sends `method` to `uri` as the owner, with no body, and answers the status.
async fn send(app: &Router, method: &str, uri: &str) -> StatusCode {
    call(app, TOKEN, method, uri, None).await.0
}

/// Moves every recent search `ms` into the past.
async fn age(db: &Database, ms: i64) {
    db.write(move |tx| {
        tx.execute("UPDATE recent_searches SET searched_at = searched_at - ?1", [ms])
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_search_is_recorded_with_what_was_selected_and_listed_newest_first() {
    let (_temp, _db, app) = app_with_tracks().await;
    let (status, first) = record(&app, json!({ "query": "  aurora  " })).await;
    assert_eq!(status, StatusCode::CREATED, "{first}");
    assert_eq!((first["query"].as_str(), &first["selected"]), (Some("aurora"), &Value::Null));
    assert!(first["searchedAt"].as_str().unwrap().ends_with('Z'));

    let (status, second) =
        record(&app, json!({ "query": "signal", "selected": { "type": "album", "id": "101" } }))
            .await;
    assert_eq!(status, StatusCode::CREATED, "{second}");
    assert_eq!(second["selected"], json!({ "type": "album", "id": "101" }));
    assert_eq!(listed(&app, TOKEN).await, ["signal", "aurora"]);
}

#[tokio::test]
async fn searched_at_is_rfc_3339_with_milliseconds() {
    let (_temp, db, app) = app_with_tracks().await;
    record(&app, json!({ "query": "aurora" })).await;
    // In the future, so the window keeps it.
    db.write(|tx| tx.execute("UPDATE recent_searches SET searched_at = 4102444799999", []))
        .await
        .unwrap();
    let (_, list) = json(&app, URI).await;
    assert_eq!(list["items"][0]["searchedAt"], "2099-12-31T23:59:59.999Z");
}

#[tokio::test]
async fn the_same_words_again_move_to_the_top_under_a_new_id() {
    let (_temp, _db, app) = app_with_tracks().await;
    let (_, first) = record(&app, json!({ "query": "Björk" })).await;
    record(&app, json!({ "query": "signal" })).await;
    let (_, again) =
        record(&app, json!({ "query": "bjork", "selected": { "type": "track", "id": "1" } })).await;
    assert_eq!(listed(&app, TOKEN).await, ["bjork", "signal"], "ignoring case and accents");
    assert_ne!(first["id"], again["id"]);
}

#[tokio::test]
async fn only_the_newest_within_the_window_are_kept() {
    let (_temp, db, app) = app_with_tracks().await;
    for n in 0..KEPT + 3 {
        record(&app, json!({ "query": format!("search {n}") })).await;
    }
    let kept = listed(&app, TOKEN).await;
    assert_eq!(kept.len(), KEPT as usize);
    assert_eq!(kept[0], format!("search {}", KEPT + 2));

    age(&db, WINDOW_MS + 1).await;
    assert!(listed(&app, TOKEN).await.is_empty(), "aged out of the window");
    record(&app, json!({ "query": "fresh" })).await;
    let swept = Arc::clone(&db);
    tokio::task::spawn_blocking(move || sweep(&swept, now_ms())).await.unwrap();
    let stored: Vec<String> = db
        .read(|conn| {
            conn.prepare("SELECT query FROM recent_searches")?
                .query_map([], |row| row.get(0))?
                .collect()
        })
        .await
        .unwrap();
    assert_eq!(stored, ["fresh"], "what aged out is deleted by the sweep, not hidden");
}

#[tokio::test]
async fn what_cannot_be_searched_or_selected_is_refused() {
    let (_temp, _db, app) = app_with_tracks().await;
    for body in [
        json!({ "query": "  ...  " }),
        json!({ "query": "x".repeat(501) }),
        json!({ "query": "a", "selected": { "type": "playlist", "id": "1" } }),
        json!({ "query": "a", "selected": { "type": "track", "id": "01" } }),
        // Track 3 is in library 2.
        json!({ "query": "a", "selected": { "type": "track", "id": "3" } }),
    ] {
        let (status, problem) = record(&app, body.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}: {problem}");
    }
    assert!(listed(&app, TOKEN).await.is_empty());
}

#[tokio::test]
async fn one_can_be_removed_or_all_cleared_and_never_anyone_elses() {
    let (temp, _db, app) = app_with_tracks().await;
    let (_, first) = record(&app, json!({ "query": "aurora" })).await;
    record(&app, json!({ "query": "signal" })).await;

    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    access(&conn, 2, 1).unwrap();
    let theirs = format!("{URI}/{}", first["id"].as_str().unwrap());
    let deleted = call(&app, "user-token", "DELETE", &theirs, None).await;
    assert_eq!(deleted.0, StatusCode::NOT_FOUND);
    assert!(listed(&app, "user-token").await.is_empty(), "each user sees only their own");

    assert_eq!(send(&app, "DELETE", &theirs).await, StatusCode::NO_CONTENT);
    assert_eq!(send(&app, "DELETE", &theirs).await, StatusCode::NOT_FOUND, "already gone");
    assert_eq!(listed(&app, TOKEN).await, ["signal"]);
    assert_eq!(send(&app, "DELETE", URI).await, StatusCode::NO_CONTENT);
    assert!(listed(&app, TOKEN).await.is_empty());
}

/// User 2 searches in library 1 from devices 5 and 6. No two of those IDs are alike, so a
/// handler that takes the device for the account, or swaps the account and the library where the
/// others do not, misses what the other device made.
#[tokio::test]
async fn a_search_is_the_accounts_on_each_of_its_devices() {
    let (temp, _db, app) = app_with_tracks().await;
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 5, "laptop-token");
    add_device(&conn, 2, 6, "phone-token");
    access(&conn, 2, 1).unwrap();
    let (laptop, phone) = ("laptop-token", "phone-token");
    let aurora = Some(json!({ "query": "aurora" }));

    let (status, recorded) = call(&app, laptop, "POST", URI, aurora.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{recorded}");
    assert_eq!(listed(&app, phone).await, ["aurora"]);
    let one = format!("{URI}/{}", recorded["id"].as_str().unwrap());
    assert_eq!(call(&app, phone, "DELETE", &one, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, laptop, "DELETE", &one, None).await.0, StatusCode::NOT_FOUND);

    assert_eq!(call(&app, phone, "POST", URI, aurora).await.0, StatusCode::CREATED);
    // Clearing them leaves the owner's alone.
    record(&app, json!({ "query": "signal" })).await;
    assert_eq!(call(&app, laptop, "DELETE", URI, None).await.0, StatusCode::NO_CONTENT);
    assert!(listed(&app, phone).await.is_empty());
    assert_eq!(listed(&app, TOKEN).await, ["signal"]);
}

#[tokio::test]
async fn a_search_is_kept_per_library() {
    let (_temp, _db, app) = app_with_tracks().await;
    record(&app, json!({ "query": "aurora" })).await;
    let (status, other) = json(&app, "/api/v1/libraries/2/recent-searches").await;
    assert_eq!((status, &other["items"]), (StatusCode::OK, &json!([])));
}

/// A search is removed only through its own library, and removing it elsewhere notes nothing,
/// even for a user whose searches a plugin is told of.
#[tokio::test]
async fn a_search_is_removed_only_in_its_library() {
    let (temp, db, app) = app_with_tracks().await;
    share_with_a_plugin(&temp, &db).await;
    let elsewhere = "/api/v1/libraries/2/recent-searches";
    let (status, theirs) =
        call(&app, TOKEN, "POST", elsewhere, Some(json!({ "query": "aurora" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{theirs}");

    let here = format!("{URI}/{}", theirs["id"].as_str().unwrap());
    assert_eq!(send(&app, "DELETE", &here).await, StatusCode::NOT_FOUND);
    let (_, kept) = json(&app, elsewhere).await;
    assert_eq!(kept["items"][0]["id"], theirs["id"]);
    let sql = "SELECT count(*) FROM search_events WHERE library_id = 1";
    let noted: i64 = db.read(move |conn| conn.query_row(sql, [], |row| row.get(0))).await.unwrap();
    assert_eq!(noted, 0);
}

/// Shares the owner's searches with `plugin`, which is allowed them, enabled in library 1, and
/// connected by the owner.
async fn share_with_a_plugin(temp: &tempfile::TempDir, db: &Arc<Database>) {
    let mut searcher = manifest("plugin", &[LibraryRead, SearchActivity, Searched]);
    searcher["personalSettings"] = json!({ "properties": { "token": { "type": "string" } } });
    install(db, searcher).await;
    let plugins = plugins(temp, db);
    grant(&plugins, "plugin", &[SearchActivity, Searched], &[LibraryRead]).await;
    enable(&plugins, "plugin", true).await;
    connect(db, "plugin", 1, json!({}), 0).await;
    db.try_write(|tx| set_search_sharing(tx, "plugin", caller(1, true), true)).await.unwrap();
}

/// What a search found: its tracks, albums, and artists.
type Found = (i64, i64, i64);

/// The events noted for the `searched` hook: each kind and search, and what a search found.
async fn events(db: &Database) -> Vec<(String, Option<i64>, Option<Found>)> {
    db.read(|conn| {
        conn.prepare(
            "SELECT kind, search_id, found_tracks, found_albums, found_artists FROM search_events \
             ORDER BY seq",
        )?
        .query_map([], |row| {
            let found: (Option<i64>, Option<i64>, Option<i64>) =
                (row.get(2)?, row.get(3)?, row.get(4)?);
            let found = match found {
                (Some(tracks), Some(albums), Some(artists)) => Some((tracks, albums, artists)),
                _ => None,
            };
            Ok((row.get(0)?, row.get(1)?, found))
        })?
        .collect()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn what_was_found_is_noted_only_for_a_user_who_shares() {
    let (temp, db, app) = app_with_tracks().await;
    install(&db, manifest("other", &[])).await;
    connect(&db, "other", 1, json!({}), 0).await;
    // Another user shares with a plugin, which says nothing of the owner.
    db.write(|tx| {
        add_user(tx, 2, "user");
        Ok(())
    })
    .await
    .unwrap();
    connect(&db, "other", 2, json!({}), 0).await;
    db.write(|tx| {
        tx.execute("UPDATE plugin_connections SET searches_shared_at = 0 WHERE user_id = 2", [])
    })
    .await
    .unwrap();
    let (_, first) = record(&app, json!({ "query": "b" })).await;
    let (_, removed) = record(&app, json!({ "query": "c" })).await;
    let removed = format!("{URI}/{}", removed["id"].as_str().unwrap());
    assert_eq!(send(&app, "DELETE", &removed).await, StatusCode::NO_CONTENT);
    assert!(
        events(&db).await.is_empty(),
        "the owner is connected, but shares with no plugin, though another user does"
    );

    share_with_a_plugin(&temp, &db).await;
    db.write(|tx| Artist::new(301, 1, "A").insert(tx)).await.unwrap();
    let (_, second) = record(&app, json!({ "query": "a" })).await;
    let id = |search: &Value| search["id"].as_str().unwrap().parse::<i64>().unwrap();
    // Replaced by the same words: not a removal, so nothing to forget.
    let (_, again) = record(&app, json!({ "query": "A" })).await;
    assert_eq!(
        send(&app, "DELETE", &format!("{URI}/{}", id(&first))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(send(&app, "DELETE", URI).await, StatusCode::NO_CONTENT);
    assert_eq!(
        events(&db).await,
        [
            ("searched".into(), Some(id(&second)), Some((2, 0, 1))),
            ("searched".into(), Some(id(&again)), Some((2, 0, 1))),
            ("forgotten".into(), Some(id(&first)), None),
            ("forgotten".into(), Some(id(&again)), None),
        ],
        "both of library 1's tracks and its one artist are called A, and its album is untitled"
    );
}

#[tokio::test]
async fn only_a_noted_search_keeps_what_it_found_and_keeps_all_of_it() {
    let (_temp, db, _app) = app_with_tracks().await;
    let insert = |found: &'static str| {
        let db = db.clone();
        async move {
            db.write(move |tx| {
                tx.execute(
                    &format!(
                        "INSERT INTO search_events (user_id, library_id, kind, search_id, \
                                                    found_tracks, found_albums, found_artists, \
                                                    created_at) VALUES (1, 1, {found}, 0)"
                    ),
                    [],
                )
            })
            .await
        }
    };
    assert!(insert("'searched', 5, 1, 1, 1").await.is_ok(), "a search, with all three counts");
    assert!(insert("'searched', 5, 1, NULL, 1").await.is_err(), "a search missing a count");
    assert!(insert("'forgotten', 5, 0, NULL, NULL").await.is_err(), "a forget with a count");
}

/// Library 1's search index is built the first time anything searches it, and a search after a
/// change answers from the index it has while another is built. So an index built while the
/// owner's search was recorded would answer the search that follows, and miss the track added
/// in between.
#[tokio::test]
async fn a_search_is_not_counted_for_a_user_who_shares_nothing() {
    let (_temp, db, app) = app_with_tracks().await;
    let (status, recorded) = record(&app, json!({ "query": "a" })).await;
    assert_eq!(status, StatusCode::CREATED, "{recorded}");
    db.write(|tx| {
        Track { title: "A".into(), sort_key: b"a".to_vec(), ..Track::new(4, 1, 101) }.insert(tx)
    })
    .await
    .unwrap();
    let (status, found) = json(&app, "/api/v1/libraries/1/search?q=a&types=tracks").await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["sections"][0]["total"], 3, "this search built the index, not the one before");
}
