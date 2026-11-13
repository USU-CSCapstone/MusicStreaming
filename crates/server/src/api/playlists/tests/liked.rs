//! Each user's Liked Songs, as playlists show it (`requirements/playlists.md` §11).

use super::super::super::testing::socket::{Socket, connect_with, next_of, serve};
use super::*;

/// The owner's second device.
const PHONE: &str = "phone";

/// The owner's Liked Songs in library 1, as listed.
async fn liked(app: &Router) -> Value {
    let (status, page) = call(app, TOKEN, "GET", URI, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    items.iter().find(|p| p["title"] == "Liked Songs").cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn it_is_listed_first_pinned_and_empty_with_no_artwork() {
    let (_temp, _db, app) = app_with_tracks().await;
    create(&app, json!({ "title": "Above it by name", "items": [{ "type": "track", "id": "1" }] }))
        .await;
    let (_, page) = call(&app, TOKEN, "GET", &format!("{URI}?sort=name"), None).await;
    let first = &page["items"][0];
    assert_eq!(
        (&first["title"], &first["pinned"], &first["trackCount"], &first["image"]),
        (&json!("Liked Songs"), &json!(true), &json!(0), &Value::Null)
    );
}

#[tokio::test]
async fn deleted_it_is_not_made_again() {
    let (_temp, db, app) = app_with_tracks().await;
    let uri = format!("{URI}/{}", liked(&app).await["id"].as_str().unwrap());
    assert_eq!(call(&app, TOKEN, "DELETE", &uri, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(liked(&app).await, Value::Null);

    // A library made since gives the owner one there, and not again in library 1.
    db.write(|tx| library(tx, 3, "Third", "/third")).await.unwrap();
    assert_eq!(liked(&app).await, Value::Null);
    let (_, third) = call(&app, TOKEN, "GET", "/api/v1/libraries/3/playlists", None).await;
    assert_eq!(third["items"][0]["title"], "Liked Songs");
}

/// Likes or unlikes track `track` in library 1 as the account `token` signs in, as `method`
/// says: `PUT` or `DELETE`.
async fn like(app: &Router, token: &str, method: &str, track: &str) -> StatusCode {
    let uri = format!("/api/v1/libraries/1/tracks/{track}/like");
    let (status, body) = call(app, token, method, &uri, None).await;
    assert!(status != StatusCode::NO_CONTENT || body == Value::Null, "{body}");
    status
}

/// The owner's Liked Songs' tracks, in its order.
async fn liked_tracks(app: &Router) -> Vec<String> {
    let uri = format!("{URI}/{}/items", liked(app).await["id"].as_str().unwrap());
    let (_, page) = call(app, TOKEN, "GET", &uri, None).await;
    let items = page["items"].as_array().unwrap();
    items.iter().map(|item| item["track"]["id"].as_str().unwrap().to_owned()).collect()
}

/// Whether tracks 1 and 2 show as liked to the account `token` signs in, as listed.
async fn shown(app: &Router, token: &str) -> [bool; 2] {
    let (_, page) = call(app, token, "GET", "/api/v1/libraries/1/tracks?ids=1,2", None).await;
    [0, 1].map(|i| page["items"][i]["liked"].as_bool().unwrap())
}

#[tokio::test]
async fn a_like_goes_to_the_top_once_and_shows_on_the_track() {
    let (_temp, _db, app) = app_with_tracks().await;
    for track in ["1", "2", "1"] {
        assert_eq!(like(&app, TOKEN, "PUT", track).await, StatusCode::NO_CONTENT);
    }
    assert_eq!(liked_tracks(&app).await, ["2", "1"], "liked again, it stays where it was");
    let playlist = liked(&app).await;
    assert_eq!((&playlist["trackCount"], &playlist["version"]), (&json!(2), &json!(3)));
    assert_eq!(shown(&app, TOKEN).await, [true, true]);
    let (_, track) = call(&app, TOKEN, "GET", "/api/v1/libraries/1/tracks/1", None).await;
    assert_eq!(track["liked"], true);
    let items = format!("{URI}/{}/items", playlist["id"].as_str().unwrap());
    let (_, page) = call(&app, TOKEN, "GET", &items, None).await;
    assert_eq!(page["items"][0]["track"]["liked"], true);

    for _ in 0..2 {
        assert_eq!(like(&app, TOKEN, "DELETE", "1").await, StatusCode::NO_CONTENT);
    }
    assert_eq!(liked_tracks(&app).await, ["2"]);
    assert_eq!(shown(&app, TOKEN).await, [false, true]);
    assert_eq!(liked(&app).await["version"], 4, "unliked again, nothing changes");
}

#[tokio::test]
async fn likes_are_each_users_own_in_each_library() {
    let (temp, _db, app) = app_with_tracks().await;
    add_others(&temp);
    // Track 3 is library 2's, so library 1 has no such track.
    assert_eq!(like(&app, TOKEN, "PUT", "3").await, StatusCode::NOT_FOUND);
    assert_eq!(like(&app, TOKEN, "PUT", "nope").await, StatusCode::NOT_FOUND);
    assert_eq!(like(&app, TOKEN, "PUT", "1").await, StatusCode::NO_CONTENT);
    assert_eq!(shown(&app, "user-token").await, [false, false], "another account's");
    // User 2 had no Liked Songs, reaching library 1 by a grant; a like makes one.
    assert_eq!(like(&app, "user-token", "PUT", "2").await, StatusCode::NO_CONTENT);
    assert_eq!(shown(&app, "user-token").await, [false, true]);
    assert_eq!(shown(&app, TOKEN).await, [true, false]);
    let (_, other) = call(&app, TOKEN, "GET", LIBRARY_2, None).await;
    assert_eq!(other["items"][0]["trackCount"], 0, "library 2's is untouched");
}

#[tokio::test]
async fn a_like_after_a_delete_makes_it_again_pinned() {
    let (_temp, db, app) = app_with_tracks().await;
    like(&app, TOKEN, "PUT", "1").await;
    let uri = format!("{URI}/{}", liked(&app).await["id"].as_str().unwrap());
    assert_eq!(call(&app, TOKEN, "DELETE", &uri, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(shown(&app, TOKEN).await, [false, false], "deleting it unliked its tracks");
    assert_eq!(like(&app, TOKEN, "DELETE", "1").await, StatusCode::NO_CONTENT, "nothing to unlike");
    assert_eq!(liked(&app).await, Value::Null);

    assert_eq!(like(&app, TOKEN, "PUT", "1").await, StatusCode::NO_CONTENT);
    let made = liked(&app).await;
    assert_eq!((&made["pinned"], &made["kind"]), (&json!(true), &json!("liked")));
    assert_eq!(liked_tracks(&app).await, ["1"]);
    let remembered: i64 = db
        .read(|conn| conn.query_row("SELECT count(*) FROM liked_songs_deleted", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(remembered, 0, "no longer deleted");
}

#[tokio::test]
async fn edited_it_keeps_its_name_and_its_adds_like() {
    let (_temp, _db, app) = app_with_tracks().await;
    like(&app, TOKEN, "PUT", "1").await;
    let playlist = liked(&app).await;
    let uri = format!("{URI}/{}", playlist["id"].as_str().unwrap());
    let base = &playlist["version"];
    for details in [json!({ "title": "Faves" }), json!({ "description": "Mine" })] {
        let mut body = details.clone();
        body["baseVersion"] = base.clone();
        let (status, problem) = call(&app, TOKEN, "PATCH", &uri, Some(body)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{details}: {problem}");
    }

    // An add with no position goes to the top, and leaves out what is liked already, each
    // track once; it can be unpinned like any playlist.
    let tracks = json!([
        { "type": "track", "id": "2" }, { "type": "track", "id": "1" }, { "type": "track", "id": "2" }
    ]);
    let body =
        json!({ "baseVersion": base, "pinned": false, "ops": [{ "op": "add", "items": tracks }] });
    let (status, edited) = call(&app, TOKEN, "PATCH", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!((&edited["pinned"], &edited["title"]), (&json!(false), &json!("Liked Songs")));
    assert_eq!(liked_tracks(&app).await, ["2", "1"]);

    // Rearranged as any playlist, and a track taken out is unliked.
    let body = json!({ "baseVersion": edited["version"], "ops": [
        { "op": "move", "positions": [0], "at": 1 }, { "op": "remove", "positions": [0] }
    ] });
    assert_eq!(call(&app, TOKEN, "PATCH", &uri, Some(body)).await.0, StatusCode::OK);
    assert_eq!(liked_tracks(&app).await, ["2"]);
    assert_eq!(shown(&app, TOKEN).await, [false, true]);
}

#[tokio::test]
async fn a_like_needs_no_version_but_an_edit_still_does() {
    let (_temp, _db, app) = app_with_tracks().await;
    let playlist = liked(&app).await;
    let uri = format!("{URI}/{}", playlist["id"].as_str().unwrap());
    // Another device unpins it, moving it on a version this one has not seen.
    let body = json!({ "baseVersion": playlist["version"], "pinned": false });
    let (status, unpinned) = call(&app, TOKEN, "PATCH", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{unpinned}");

    assert_eq!(like(&app, TOKEN, "PUT", "1").await, StatusCode::NO_CONTENT);
    assert_eq!(like(&app, TOKEN, "PUT", "2").await, StatusCode::NO_CONTENT);
    assert_eq!(liked_tracks(&app).await, ["2", "1"]);

    // An edit made on the version before the likes still conflicts, since it names positions.
    let current = unpinned["version"].as_i64().unwrap() + 2;
    let body = json!({ "baseVersion": unpinned["version"], "ops": [
        { "op": "remove", "positions": [0] }
    ] });
    let (status, problem) = call(&app, TOKEN, "PATCH", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{problem}");
    assert_eq!(
        (&problem["code"], &problem["currentVersion"]),
        (&json!("version_conflict"), &json!(current))
    );
    assert_eq!(liked_tracks(&app).await, ["2", "1"]);
}

/// A server with the catalog and the owner's phone, and the phone listening.
async fn listening() -> (tempfile::TempDir, Router, Socket) {
    let (temp, _db, app) = app_with_tracks().await;
    add_device(&conn(&temp), 1, 2, PHONE);
    let phone = connect_with(serve(app.clone()).await, PHONE).await;
    (temp, app, phone)
}

#[tokio::test]
async fn likes_and_unlikes_reach_the_other_devices_as_edits() {
    let (_temp, app, mut phone) = listening().await;
    let id = liked(&app).await["id"].clone();
    for track in ["1", "2"] {
        like(&app, TOKEN, "PUT", track).await;
    }
    like(&app, TOKEN, "DELETE", "1").await;

    for (version, track) in [(2, "1"), (3, "2")] {
        let message = next_of(&mut phone, "playlist").await;
        assert_eq!((&message["playlistId"], &message["version"]), (&id, &json!(version)));
        let ops = &message["ops"];
        assert_eq!((&ops[0]["op"], &ops[0]["at"]), (&json!("add"), &json!(0)), "{message}");
        assert_eq!(ops[0]["trackIds"], json!([track]));
        assert_eq!(ops[0]["addedAt"], message["playlist"]["modifiedAt"]);
    }
    let message = next_of(&mut phone, "playlist").await;
    assert_eq!(message["version"], 4);
    assert_eq!(message["ops"], json!([{ "op": "remove", "positions": [1] }]), "{message}");
    assert_eq!(message["playlist"]["trackCount"], 1);
}

#[tokio::test]
async fn a_like_that_makes_it_again_brings_it_to_the_other_devices() {
    let (_temp, app, mut phone) = listening().await;
    let old = liked(&app).await["id"].clone();
    let uri = format!("{URI}/{}", old.as_str().unwrap());
    call(&app, TOKEN, "DELETE", &uri, None).await;
    let deleted = next_of(&mut phone, "playlist").await;
    assert_eq!((&deleted["playlistId"], &deleted["playlist"]), (&old, &Value::Null));

    like(&app, TOKEN, "PUT", "1").await;
    // The phone never heard it was made: the message carries the whole playlist to show.
    let message = next_of(&mut phone, "playlist").await;
    let made = liked(&app).await;
    assert_ne!(made["id"], old);
    assert_eq!(message["playlistId"], made["id"]);
    assert_eq!(message["playlist"], made, "{message}");
    assert_eq!((&made["kind"], &made["pinned"]), (&json!("liked"), &json!(true)));
    assert_eq!(message["ops"][0]["trackIds"], json!(["1"]));
}

#[tokio::test]
async fn a_like_makes_liked_songs_again_under_the_id_the_client_chose() {
    let (_temp, _db, app) = app_with_tracks().await;
    let first = liked(&app).await["id"].as_str().unwrap().to_owned();
    let other =
        create(&app, json!({ "title": "Another" })).await["id"].as_str().unwrap().to_owned();
    let uri =
        |track: &str, id: &str| format!("/api/v1/libraries/1/tracks/{track}/like?likedId={id}");

    // While Liked Songs is there, the ID is not used, nor checked.
    let (status, _) = call(&app, TOKEN, "PUT", &uri("1", &other), None).await;
    assert_eq!(
        (status, liked(&app).await["id"].as_str().unwrap()),
        (StatusCode::NO_CONTENT, &*first)
    );

    assert_eq!(
        call(&app, TOKEN, "DELETE", &format!("{URI}/{first}"), None).await.0,
        StatusCode::NO_CONTENT
    );
    let (status, body) = call(&app, TOKEN, "PUT", &uri("1", &other), None).await;
    assert_eq!((status, &body["code"]), (StatusCode::CONFLICT, &json!("id_taken")), "{body}");
    let (status, _) = call(&app, TOKEN, "PUT", &uri("1", "0"), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(liked(&app).await, Value::Null, "a refused like makes nothing");

    let (status, _) = call(&app, TOKEN, "PUT", &uri("2", "4611686018427387904"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(liked(&app).await["id"], "4611686018427387904");
    assert_eq!(liked_tracks(&app).await, ["2"]);
}
