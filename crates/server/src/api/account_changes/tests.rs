use axum::Router;
use axum::http::StatusCode;
use jewelcase_catalog::sync::{Kind, Rows, apply, delete};
use rusqlite::Connection;
use serde_json::{Value, json};

use super::super::testing::{TOKEN, add_device, add_user, app_with_tracks, call, conn, saved_only};
use crate::db::testing::access;

/// Library 1 with tracks 1 and 2, and library 2 with track 3, the owner (user 1) on device 1,
/// and user 2 who reaches library 1 with the token `user-token`; no Liked Songs, so the feed
/// holds only what a test makes.
async fn world() -> (tempfile::TempDir, Router) {
    let (temp, _, app) = app_with_tracks().await;
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    access(&conn, 2, 1).unwrap();
    saved_only(&conn).unwrap();
    (temp, app)
}

/// Creates a playlist in library 1 as `token` holding `tracks`, and returns its ID.
async fn create(app: &Router, token: &str, title: &str, tracks: &[i64]) -> String {
    let items: Vec<Value> =
        tracks.iter().map(|id| json!({ "type": "track", "id": id.to_string() })).collect();
    let body = json!({ "title": title, "items": items });
    let (status, playlist) =
        call(app, token, "POST", "/api/v1/libraries/1/playlists", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{playlist}");
    playlist["id"].as_str().unwrap().to_owned()
}

/// Edits playlist `id` of the owner's, which is at `version`, with `ops`.
async fn edit(app: &Router, id: &str, version: i64, ops: Value) {
    let uri = format!("/api/v1/libraries/1/playlists/{id}");
    let body = json!({ "baseVersion": version, "ops": ops });
    let (status, answer) = call(app, TOKEN, "PATCH", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
}

/// The feed `uri` of `token`.
async fn feed(app: &Router, token: &str, query: &str) -> (StatusCode, Value) {
    call(app, token, "GET", &format!("/api/v1/me/changes{query}"), None).await
}

/// The `(kind, id, key)` of each change in `page`.
fn names(page: &Value) -> Vec<(String, String, Option<i64>)> {
    page["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["kind"].as_str().unwrap().to_owned(),
                c["id"].as_str().unwrap().to_owned(),
                c["key"].as_i64(),
            )
        })
        .collect()
}

#[tokio::test]
async fn another_accounts_playlists_never_appear() {
    let (_temp, app) = world().await;
    let mine = create(&app, TOKEN, "Mine", &[1]).await;
    let theirs = create(&app, "user-token", "Theirs", &[1, 2]).await;

    for (token, own, other) in [(TOKEN, &mine, &theirs), ("user-token", &theirs, &mine)] {
        let (status, page) = feed(&app, token, "").await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let ids: Vec<String> = names(&page).into_iter().map(|(_, id, _)| id).collect();
        assert!(ids.contains(own) && !ids.contains(other), "{page}");
        let text = page.to_string();
        assert!(!text.contains(if token == TOKEN { "Theirs" } else { "Mine" }), "{text}");
        assert!(!text.contains("user_id"), "no owner column");
    }
    let (_, page) = feed(&app, TOKEN, "").await;
    assert_eq!(names(&page).len(), 2, "the playlist and its one item");
    assert_eq!(page["head"], page["next"]);
}

#[tokio::test]
async fn playlists_in_a_library_the_caller_lost_access_to_are_left_out() {
    let (temp, app) = world().await;
    let theirs = create(&app, "user-token", "Theirs", &[1]).await;
    let (_, before) = feed(&app, "user-token", "").await;
    assert_eq!(names(&before).len(), 2);

    conn(&temp).execute("DELETE FROM library_access WHERE user_id = 2", []).unwrap();
    let (status, page) = feed(&app, "user-token", "").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(names(&page), [], "no playlist and no tombstone of a library they cannot reach");
    assert_eq!((&page["next"], &page["head"]), (&before["head"], &before["head"]));

    access(&conn(&temp), 2, 1).unwrap();
    let (_, again) = feed(&app, "user-token", "").await;
    assert!(names(&again).iter().all(|(_, id, _)| *id == theirs));
    assert_eq!(names(&again).len(), 2, "regained access lists them again");
}

#[tokio::test]
async fn one_item_edit_catches_up_as_one_row() {
    let (_temp, app) = world().await;
    let id = create(&app, TOKEN, "Mix", &[1, 2, 1, 2]).await;
    let (_, all) = feed(&app, TOKEN, "").await;
    let head = all["head"].as_i64().unwrap();
    assert_eq!(names(&all).len(), 5);

    edit(&app, &id, 1, json!([{ "op": "remove", "positions": [1] }])).await;

    let (_, page) = feed(&app, TOKEN, &format!("?after={head}")).await;
    let changes = names(&page);
    assert_eq!(changes.len(), 2, "{page}");
    assert_eq!(changes.iter().filter(|c| c.0 == "playlist").count(), 1);
    let item = page["changes"].as_array().unwrap().iter().find(|c| c["kind"] == "playlistItem");
    let item = item.unwrap();
    assert_eq!((&item["op"], item.get("rows")), (&json!("delete"), None), "{item}");
    assert_eq!(item["id"], json!(id));

    edit(&app, &id, 2, json!([{ "op": "add", "items": [{ "type": "track", "id": "2" }] }])).await;
    let (_, page) = feed(&app, TOKEN, &format!("?after={}", page["head"])).await;
    let added = page["changes"].as_array().unwrap().iter().filter(|c| c["kind"] == "playlistItem");
    let added: Vec<_> = added.collect();
    assert_eq!(added.len(), 1);
    let row = &added[0]["rows"]["playlist_items"][0];
    assert_eq!((&row["track_id"], &row["playlist_id"]), (&json!("2"), &json!(id)));
    assert_eq!(row["key"], added[0]["key"]);
}

#[tokio::test]
async fn pages_end_at_head_and_a_cursor_past_it_is_gone() {
    let (_temp, app) = world().await;
    create(&app, TOKEN, "Mix", &[1, 2]).await;
    let (_, all) = feed(&app, TOKEN, "").await;
    let head = all["head"].as_i64().unwrap();

    let (_, first) = feed(&app, TOKEN, "?limit=2").await;
    assert_eq!(names(&first).len(), 2);
    assert_ne!(first["next"], first["head"]);
    let (_, last) = feed(&app, TOKEN, &format!("?after={}&limit=2", first["next"])).await;
    assert_eq!((names(&last).len(), &last["next"]), (1, &json!(head)));

    let (status, current) = feed(&app, TOKEN, &format!("?after={head}")).await;
    assert_eq!(
        (status, names(&current).len(), &current["next"]),
        (StatusCode::OK, 0, &json!(head))
    );
    let (status, gone) = feed(&app, TOKEN, &format!("?after={}", head + 1)).await;
    assert_eq!((status, &gone["code"]), (StatusCode::GONE, &json!("cursor_expired")), "{gone}");
    let (status, _) = feed(&app, "user-token", "?after=1").await;
    assert_eq!(status, StatusCode::GONE, "another account's positions are not this one's");
    let (status, _) = feed(&app, TOKEN, "?after=-1").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

/// The rows of `table` on `conn`, ordered, as text.
fn dump(conn: &Connection, table: &str) -> Vec<String> {
    let cells = if table == "playlists" {
        "id, title, version, track_count, sort_key, folded, image_key"
    } else {
        "playlist_id, key, track_id, added_at, library_id"
    };
    let columns: Vec<_> = cells.split(", ").map(|c| format!("quote({c})")).collect();
    let sql = format!("SELECT {} FROM {table} ORDER BY 1", columns.join(" || ',' || "));
    conn.prepare(&sql).unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

#[tokio::test]
async fn the_feed_brings_a_device_current_and_a_tombstone_clears_a_playlist() {
    let (temp, app) = world().await;
    let id = create(&app, TOKEN, "Mix", &[1, 2, 1]).await;
    let device = Connection::open_in_memory().unwrap();
    jewelcase_catalog::create_schema(&device).unwrap();
    let server = conn(&temp);

    // Pulls everything after `after` into the device and returns the feed's head.
    let catch_up = async |after: i64| {
        let page = feed(&app, TOKEN, &format!("?after={after}")).await.1;
        let tx = device.unchecked_transaction().unwrap();
        for change in page["changes"].as_array().unwrap() {
            let kind =
                if change["kind"] == "playlist" { Kind::Playlist } else { Kind::PlaylistItem };
            let id = change["id"].as_str().unwrap().to_owned();
            let key = change["key"].as_i64().unwrap_or(0);
            if change["op"] == "delete" {
                delete(&tx, kind, id.parse().unwrap(), key).unwrap();
            } else {
                let tables = serde_json::from_value(change["rows"].clone()).unwrap();
                apply(&tx, &Rows { kind, id, key, tables }).unwrap();
            }
        }
        tx.commit().unwrap();
        for table in ["playlists", "playlist_items"] {
            assert_eq!(dump(&device, table), dump(&server, table), "{table}");
        }
        page["head"].as_i64().unwrap()
    };
    let head = catch_up(0).await;
    assert_eq!(dump(&device, "playlist_items").len(), 3);

    // A remove arrives as an item tombstone, a move as a tombstone and a new key.
    edit(&app, &id, 1, json!([{ "op": "remove", "positions": [1] }])).await;
    let head = catch_up(head).await;
    assert_eq!(dump(&device, "playlist_items").len(), 2);
    edit(&app, &id, 2, json!([{ "op": "move", "positions": [1], "at": 0 }])).await;
    let head = catch_up(head).await;
    assert_eq!(dump(&device, "playlist_items").len(), 2);

    let uri = format!("/api/v1/libraries/1/playlists/{id}");
    let (status, _) = call(&app, TOKEN, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let page = feed(&app, TOKEN, &format!("?after={head}")).await.1;
    assert_eq!(names(&page), [("playlist".to_owned(), id.clone(), None)]);
    assert_eq!(page["changes"][0]["op"], "delete");
    catch_up(head).await;
    assert_eq!((dump(&device, "playlists").len(), dump(&device, "playlist_items").len()), (0, 0));
}

#[tokio::test]
async fn likes_and_hides_reach_a_device_and_another_accounts_never_do() {
    let (temp, _, app) = app_with_tracks().await;
    let server = conn(&temp);
    add_user(&server, 2, "user");
    add_device(&server, 2, 2, "user-token");
    access(&server, 2, 1).unwrap();
    let mark = async |token: &str, method: &str, path: &str| {
        let uri = format!("/api/v1/libraries/1/tracks/{path}");
        let (status, body) = call(&app, token, method, &uri, None).await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{method} {uri}: {body}");
    };
    mark(TOKEN, "PUT", "1/like").await;
    mark(TOKEN, "PUT", "2/hidden").await;
    mark("user-token", "PUT", "1/hidden").await;

    let device = Connection::open_in_memory().unwrap();
    jewelcase_catalog::create_schema(&device).unwrap();
    // Pulls everything after `after` into the device and returns the feed's head.
    let catch_up = async |after: i64| {
        let page = feed(&app, TOKEN, &format!("?after={after}")).await.1;
        for change in page["changes"].as_array().unwrap() {
            let kind: Kind = serde_json::from_value(change["kind"].clone()).unwrap();
            let id = change["id"].as_str().unwrap().to_owned();
            let key = change["key"].as_i64().unwrap_or(0);
            if change["op"] == "delete" {
                delete(&device, kind, id.parse().unwrap(), key).unwrap();
            } else {
                let tables = serde_json::from_value(change["rows"].clone()).unwrap();
                apply(&device, &Rows { kind, id, key, tables }).unwrap();
            }
        }
        page["head"].as_i64().unwrap()
    };
    let rows = |conn: &Connection, sql: &str| -> Vec<(i64, i64)> {
        conn.prepare(sql)
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let hidden = "SELECT user_id, track_id FROM hidden_tracks ORDER BY 1, 2";
    // The device holds the account's own playlists alone, so its likes are user 1's.
    let liked = "SELECT 1, i.track_id FROM playlist_items i \
                 JOIN playlists p ON p.id = i.playlist_id AND p.kind = 'liked' ORDER BY 1, 2";

    let head = catch_up(0).await;
    assert_eq!(rows(&device, hidden), [(1, 2)], "the owner's hide, and not user 2's");
    assert_eq!(rows(&device, liked), [(1, 1)]);
    assert_eq!(rows(&server, hidden), [(1, 2), (2, 1)]);

    // Liking a hidden track unhides it, which the device hears as the hide's tombstone.
    mark(TOKEN, "PUT", "2/like").await;
    let head = catch_up(head).await;
    assert_eq!(rows(&device, hidden), []);
    assert_eq!(rows(&device, liked), [(1, 1), (1, 2)]);
    // And hiding a liked one unlikes it.
    mark(TOKEN, "PUT", "1/hidden").await;
    catch_up(head).await;
    assert_eq!(rows(&device, hidden), [(1, 1)]);
    assert_eq!(rows(&device, liked), [(1, 2)]);
}
