//! Every path that likes, unlikes, hides, or unhides a track records what the track now is, once,
//! for that user alone, and nothing else records anything.

use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::api::testing::{TOKEN, add_device, add_user, app_with_tracks, call, conn};
use crate::db::testing::access;

/// The marks recorded after `seq`, as (user, library, track, kind), and the last seq read.
fn recorded(temp: &tempfile::TempDir, seq: &mut i64) -> Vec<(i64, i64, i64, String)> {
    let conn = conn(temp);
    let mut statement = conn
        .prepare(
            "SELECT seq, user_id, library_id, track_id, kind FROM mark_events WHERE seq > ?1 \
             ORDER BY seq",
        )
        .unwrap();
    let rows: Vec<(i64, (i64, i64, i64, String))> = statement
        .query_map([*seq], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    if let Some((last, _)) = rows.last() {
        *seq = *last;
    }
    rows.into_iter().map(|(_, row)| row).collect()
}

/// The owner's marks of track `track` of library 1, as (track, kind), for short asserts.
fn marks(rows: Vec<(i64, i64, i64, String)>) -> Vec<(i64, &'static str)> {
    rows.into_iter()
        .map(|(user, library, track, kind)| {
            assert_eq!((user, library), (1, 1));
            (track, kind.leak() as &'static str)
        })
        .collect()
}

/// Sends `method` to `uri` under library 1 as the owner, and expects it to succeed.
async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>) -> Value {
    let (status, answer) =
        call(app, TOKEN, method, &format!("/api/v1/libraries/1{uri}"), body).await;
    assert!(status.is_success(), "{method} {uri}: {status} {answer}");
    answer
}

/// The owner's Liked Songs in library 1, as its ID and version.
async fn liked_songs(app: &Router) -> (String, Value) {
    let lists = send(app, "GET", "/playlists", None).await;
    let liked = lists["items"].as_array().unwrap().iter().find(|p| p["kind"] == "liked").unwrap();
    (liked["id"].as_str().unwrap().to_owned(), liked["version"].clone())
}

#[tokio::test]
async fn each_like_and_hide_records_what_the_track_now_is() {
    let (temp, _db, app) = app_with_tracks().await;
    let mut seq = 0;
    send(&app, "PUT", "/tracks/1/like", None).await;
    send(&app, "PUT", "/tracks/1/like", None).await;
    assert_eq!(marks(recorded(&temp, &mut seq)), [(1, "liked")], "liked once");

    // Hiding a liked track unlikes it, and liking it again unhides it: the undo first each time,
    // so a plugin applying them in order ends on what was asked for.
    send(&app, "PUT", "/tracks/1/hidden", None).await;
    assert_eq!(marks(recorded(&temp, &mut seq)), [(1, "unliked"), (1, "hidden")]);
    send(&app, "PUT", "/tracks/1/like", None).await;
    assert_eq!(marks(recorded(&temp, &mut seq)), [(1, "unhidden"), (1, "liked")]);

    for _ in 0..2 {
        send(&app, "DELETE", "/tracks/1/like", None).await;
        send(&app, "DELETE", "/tracks/2/hidden", None).await;
    }
    assert_eq!(marks(recorded(&temp, &mut seq)), [(1, "unliked")], "what changed, once");
    send(&app, "PUT", "/tracks/2/hidden", None).await;
    send(&app, "DELETE", "/tracks/2/hidden", None).await;
    assert_eq!(marks(recorded(&temp, &mut seq)), [(2, "hidden"), (2, "unhidden")]);
}

#[tokio::test]
async fn edits_of_liked_songs_record_likes_and_unlikes_and_nothing_else_does() {
    let (temp, _db, app) = app_with_tracks().await;
    send(&app, "PUT", "/tracks/2/hidden", None).await;
    let mut seq = 0;
    recorded(&temp, &mut seq);

    let (id, version) = liked_songs(&app).await;
    let uri = format!("/playlists/{id}");
    let tracks = json!([{ "type": "track", "id": "1" }, { "type": "track", "id": "2" }]);
    let add = json!({ "baseVersion": version, "ops": [{ "op": "add", "items": tracks }] });
    let edited = send(&app, "PATCH", &uri, Some(add)).await;
    assert_eq!(
        marks(recorded(&temp, &mut seq)),
        [(2, "unhidden"), (1, "liked"), (2, "liked")],
        "an add unhides, then likes"
    );

    // A move keeps every track liked, so it records nothing.
    let order = json!({ "baseVersion": edited["version"], "ops": [
        { "op": "move", "positions": [0], "at": 1 }
    ] });
    let moved = send(&app, "PATCH", &uri, Some(order)).await;
    assert_eq!(recorded(&temp, &mut seq), []);
    let remove = json!({ "baseVersion": moved["version"], "ops": [
        { "op": "remove", "positions": [0] }
    ] });
    send(&app, "PATCH", &uri, Some(remove)).await;
    assert_eq!(marks(recorded(&temp, &mut seq)), [(2, "unliked")]);

    // Another playlist's edits, and its deletion, like nothing.
    let body = json!({ "title": "Mix", "items": tracks });
    let mix = send(&app, "POST", "/playlists", Some(body)).await;
    let mix_uri = format!("/playlists/{}", mix["id"].as_str().unwrap());
    let remove = json!({ "baseVersion": mix["version"], "ops": [
        { "op": "remove", "positions": [0] }
    ] });
    send(&app, "PATCH", &mix_uri, Some(remove)).await;
    send(&app, "DELETE", &mix_uri, None).await;
    assert_eq!(recorded(&temp, &mut seq), []);
}

#[tokio::test]
async fn deleting_liked_songs_unlikes_everything_in_it_for_that_user_alone() {
    let (temp, _db, app) = app_with_tracks().await;
    {
        let conn = conn(&temp);
        add_user(&conn, 2, "user");
        add_device(&conn, 2, 2, "user-token");
        access(&conn, 2, 1).unwrap();
    }
    send(&app, "PUT", "/tracks/1/like", None).await;
    send(&app, "PUT", "/tracks/2/like", None).await;
    let (status, _) =
        call(&app, "user-token", "PUT", "/api/v1/libraries/1/tracks/1/like", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let mut seq = 0;
    recorded(&temp, &mut seq);

    let (id, _) = liked_songs(&app).await;
    send(&app, "DELETE", &format!("/playlists/{id}"), None).await;
    // In the playlist's order: the newest like at the top.
    assert_eq!(marks(recorded(&temp, &mut seq)), [(2, "unliked"), (1, "unliked")]);

    // User 2's like stands, recorded as theirs.
    let (status, theirs) =
        call(&app, "user-token", "GET", "/api/v1/libraries/1/tracks?ids=1", None).await;
    assert_eq!((status, &theirs["items"][0]["liked"]), (StatusCode::OK, &json!(true)));
    let conn = conn(&temp);
    let users: Vec<i64> = conn
        .prepare("SELECT user_id FROM mark_events ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(users, [1, 1, 2, 1, 1]);
}
