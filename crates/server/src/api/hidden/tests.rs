//! Hiding a track: Hidden Songs, each user's own, and liking and hiding undoing each other.

use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::testing::socket::{connect_with, next_of, serve};
use super::super::testing::{TOKEN, add_device, add_user, app_with_tracks, call, conn};
use crate::db::testing::access;

/// Hides or unhides track `track` of library 1 as `token`'s account, as `method` says: `PUT` or
/// `DELETE`.
async fn hide(app: &Router, token: &str, method: &str, track: &str) -> StatusCode {
    let uri = format!("/api/v1/libraries/1/tracks/{track}/hidden");
    call(app, token, method, &uri, None).await.0
}

/// The owner's Hidden Songs in library 1 for `query`, as track IDs, and the total.
async fn hidden(app: &Router, token: &str, query: &str) -> (Vec<String>, Value) {
    let uri = format!("/api/v1/libraries/1/hidden-tracks{query}");
    let (status, page) = call(app, token, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    let ids = items.iter().map(|track| track["id"].as_str().unwrap().to_owned()).collect();
    (ids, page["total"].clone())
}

/// Whether track 1 is liked by the owner, as listed.
async fn liked(app: &Router) -> bool {
    let (_, page) = call(app, TOKEN, "GET", "/api/v1/libraries/1/tracks?ids=1", None).await;
    page["items"][0]["liked"].as_bool().unwrap()
}

#[tokio::test]
async fn hidden_songs_lists_what_was_hidden_newest_first() {
    let (_temp, _db, app) = app_with_tracks().await;
    for track in ["1", "2", "1"] {
        assert_eq!(hide(&app, TOKEN, "PUT", track).await, StatusCode::NO_CONTENT);
    }
    assert_eq!(hidden(&app, TOKEN, "").await, (vec!["2".into(), "1".into()], json!(2)));
    assert_eq!(hidden(&app, TOKEN, "?order=asc").await.0, ["1", "2"]);
    let (_, first) =
        call(&app, TOKEN, "GET", "/api/v1/libraries/1/hidden-tracks?limit=1", None).await;
    let cursor = first["nextCursor"].as_str().unwrap();
    let rest = hidden(&app, TOKEN, &format!("?limit=1&cursor={cursor}")).await;
    assert_eq!(rest.0, ["1"]);

    for _ in 0..2 {
        assert_eq!(hide(&app, TOKEN, "DELETE", "2").await, StatusCode::NO_CONTENT);
    }
    assert_eq!(hidden(&app, TOKEN, "").await.0, ["1"]);
}

#[tokio::test]
async fn hiding_is_each_users_own_in_each_library() {
    let (temp, _db, app) = app_with_tracks().await;
    {
        let conn = conn(&temp);
        add_user(&conn, 2, "user");
        add_device(&conn, 2, 2, "user-token");
        access(&conn, 2, 1).unwrap();
    }
    // Track 3 is library 2's, so library 1 has no such track.
    assert_eq!(hide(&app, TOKEN, "PUT", "3").await, StatusCode::NOT_FOUND);
    assert_eq!(hide(&app, TOKEN, "PUT", "1").await, StatusCode::NO_CONTENT);
    assert_eq!(hidden(&app, "user-token", "").await.0, Vec::<String>::new());
    let (_, other) = call(&app, TOKEN, "GET", "/api/v1/libraries/2/hidden-tracks", None).await;
    assert_eq!(other["items"], json!([]));
}

#[tokio::test]
async fn liking_and_hiding_undo_each_other() {
    let (_temp, _db, app) = app_with_tracks().await;
    let like = "/api/v1/libraries/1/tracks/1/like";
    assert_eq!(call(&app, TOKEN, "PUT", like, None).await.0, StatusCode::NO_CONTENT);
    assert!(liked(&app).await);
    hide(&app, TOKEN, "PUT", "1").await;
    assert!(!liked(&app).await, "hiding unlikes it");
    assert_eq!(hidden(&app, TOKEN, "").await.0, ["1"]);
    call(&app, TOKEN, "PUT", like, None).await;
    assert!(liked(&app).await);
    assert_eq!(hidden(&app, TOKEN, "").await.0, Vec::<String>::new(), "liking unhides it");

    // Added to Liked Songs by an edit, as a like is, it is unhidden too.
    hide(&app, TOKEN, "PUT", "2").await;
    let (_, lists) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/playlists", None).await;
    let liked_songs = &lists["items"][0];
    let uri = format!("/api/v1/libraries/1/playlists/{}", liked_songs["id"].as_str().unwrap());
    let add = json!({ "baseVersion": liked_songs["version"], "ops": [
        { "op": "add", "items": [{ "type": "track", "id": "2" }] }
    ] });
    assert_eq!(call(&app, TOKEN, "PATCH", &uri, Some(add)).await.0, StatusCode::OK);
    assert_eq!(hidden(&app, TOKEN, "").await.0, Vec::<String>::new());
}

#[tokio::test]
async fn a_hide_reaches_the_users_devices() {
    let (temp, _db, app) = app_with_tracks().await;
    add_device(&conn(&temp), 1, 2, "phone");
    let address = serve(app.clone()).await;
    let mut phone = connect_with(address, "phone").await;
    hide(&app, TOKEN, "PUT", "1").await;
    let message = next_of(&mut phone, "trackHidden").await;
    assert_eq!(
        message,
        json!({ "type": "trackHidden", "libraryId": "1", "trackId": "1", "hidden": true })
    );
    hide(&app, TOKEN, "DELETE", "1").await;
    assert_eq!(next_of(&mut phone, "trackHidden").await["hidden"], false);
}

/// A hide keeps no missing track, so a purge takes the track's hide with it
/// (`requirements/scanning.md` §8).
#[tokio::test]
async fn a_purged_track_takes_its_hide() {
    let (_temp, db, app) = app_with_tracks().await;
    hide(&app, TOKEN, "PUT", "1").await;
    let left: i64 = db
        .write(|tx| {
            tx.execute("DELETE FROM tracks WHERE id = 1", [])?;
            tx.query_row("SELECT count(*) FROM hidden_tracks", [], |row| row.get(0))
        })
        .await
        .unwrap();
    assert_eq!(left, 0);
}

/// The IDs of library 1's tracks a list shows, a page of `limit` at a time, and each page's
/// total, for `query`.
async fn shown(app: &Router, query: &str, limit: usize) -> (Vec<String>, Vec<i64>) {
    let (mut ids, mut totals, mut cursor) = (Vec::new(), Vec::new(), None::<String>);
    loop {
        let after = cursor.map(|c| format!("&cursor={c}")).unwrap_or_default();
        let uri = format!("/api/v1/libraries/1/tracks?limit={limit}{query}{after}");
        let (status, page) = call(app, TOKEN, "GET", &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {page}");
        for track in page["items"].as_array().unwrap() {
            ids.push(track["id"].as_str().unwrap().to_owned());
        }
        totals.push(page["total"].as_i64().unwrap());
        match page["nextCursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => return (ids, totals),
        }
    }
}

/// A hidden track is in no list of tracks, and no list counts it: the library's, an album's,
/// an artist's (read from their credits), a tag's (one alone takes its stored count), and
/// search. Unhidden, it is back.
#[tokio::test]
async fn a_hidden_track_is_left_out_of_every_list_and_count() {
    let (_temp, _db, app) = super::super::testing::catalog::app().await;
    let lists = ["", "&albumId=101", "&artistId=10", "&tagAll=30", "&sort=name&order=desc"];
    let mut before = Vec::new();
    for query in lists {
        before.push(shown(&app, query, 100).await);
    }
    assert!(before.iter().all(|(ids, _)| ids.contains(&"1001".to_owned())));

    hide(&app, TOKEN, "PUT", "1001").await;
    for (query, (ids, totals)) in lists.into_iter().zip(&before) {
        for limit in [1, 100] {
            let (now, counted) = shown(&app, query, limit).await;
            let without: Vec<_> = ids.iter().filter(|id| *id != "1001").cloned().collect();
            assert_eq!(now, without, "{query} by {limit}");
            assert!(counted.iter().all(|total| *total == totals[0] - 1), "{query}: {counted:?}");
        }
    }
    let (_, found) =
        call(&app, TOKEN, "GET", "/api/v1/libraries/1/search?q=come%20together", None).await;
    let tracks = found["sections"].as_array().unwrap().iter().find(|s| s["type"] == "tracks");
    assert_eq!(tracks, None, "{found}");

    hide(&app, TOKEN, "DELETE", "1001").await;
    for (query, listed) in lists.into_iter().zip(&before) {
        assert_eq!(&shown(&app, query, 100).await, listed, "{query}");
    }
}

/// A playlist keeps a hidden track in its place, marked, so a device leaves it out of what it
/// shows while its positions stay the playlist's.
#[tokio::test]
async fn a_playlist_marks_a_hidden_track_in_its_place() {
    let (_temp, _db, app) = app_with_tracks().await;
    let items = json!([{ "type": "track", "id": "1" }, { "type": "track", "id": "2" }]);
    let (_, made) = call(
        &app,
        TOKEN,
        "POST",
        "/api/v1/libraries/1/playlists",
        Some(json!({ "title": "Mix", "items": items })),
    )
    .await;
    hide(&app, TOKEN, "PUT", "1").await;
    let uri = format!("/api/v1/libraries/1/playlists/{}/items", made["id"].as_str().unwrap());
    let (_, page) = call(&app, TOKEN, "GET", &uri, None).await;
    let marks: Vec<_> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| (item["track"]["id"].clone(), item["hidden"].clone()))
        .collect();
    assert_eq!(marks, [(json!("1"), json!(true)), (json!("2"), json!(false))]);
    assert_eq!(page["total"], 2, "its positions are the playlist's");
}
