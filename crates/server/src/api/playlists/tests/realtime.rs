//! The `playlist` realtime message: sent to the owner's sockets once a change is committed, and
//! to no one else.

use std::time::Duration;

use super::super::super::testing::socket::{Socket, connect_with, next_of, serve};
use super::edit::{edit, send, track};
use super::*;

/// The owner's second device and another user, each with a token.
const PHONE: &str = "phone";
const STRANGER: &str = "stranger";

/// A server with the catalog, the owner's phone, and a second user; the directory holds its data.
async fn world() -> (tempfile::TempDir, Router, std::net::SocketAddr) {
    let (temp, _, app) = catalog().await;
    let conn = conn(&temp);
    add_device(&conn, 1, 2, PHONE);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 3, STRANGER);
    let address = serve(app.clone()).await;
    (temp, app, address)
}

/// The next `playlist` message.
async fn change(socket: &mut Socket) -> Value {
    next_of(socket, "playlist").await
}

/// Asserts no `playlist` message arrives within a moment.
async fn quiet(socket: &mut Socket) {
    let waited = tokio::time::timeout(Duration::from_millis(300), change(socket)).await;
    assert!(waited.is_err(), "unexpected message: {waited:?}");
}

#[tokio::test]
async fn an_edit_reaches_the_other_device_with_its_ops_and_version() {
    let (_temp, app, address) = world().await;
    let id = create(&app, json!({ "title": "Mix", "items": [track(1)] })).await["id"].clone();
    let mut desk = connect_with(address, TOKEN).await;
    let mut phone = connect_with(address, PHONE).await;
    let mut stranger = connect_with(address, STRANGER).await;

    let ops = json!([
        { "op": "add", "items": [track(2), { "type": "album", "id": "102" }], "at": 1 },
        { "op": "move", "positions": [0], "at": 2 },
        { "op": "remove", "positions": [3] },
    ]);
    let (status, edited) =
        edit(&app, id.as_str().unwrap(), json!({ "baseVersion": 1, "ops": ops })).await;
    assert_eq!(status, StatusCode::OK, "{edited}");

    for socket in [&mut phone, &mut desk] {
        let message = change(socket).await;
        assert_eq!(message["libraryId"], json!("1"));
        assert_eq!(message["playlistId"], id);
        assert_eq!(message["version"], json!(2));
        assert_eq!(message["playlist"], edited, "{message}");
        assert_eq!(message.get("refetch"), None);
        let ops = message["ops"].as_array().unwrap();
        assert_eq!(ops.len(), 3, "{message}");
        // Albums are expanded: the add names tracks, in the order they were put.
        assert_eq!(ops[0]["op"], json!("add"));
        assert_eq!(ops[0]["at"], json!(1));
        assert_eq!(ops[0]["trackIds"], json!(["2", "13", "12", "14", "11"]));
        assert_eq!(ops[0]["addedAt"], edited["modifiedAt"]);
        assert_eq!(ops[1], json!({ "op": "move", "positions": [0], "at": 2 }));
        assert_eq!(ops[2], json!({ "op": "remove", "positions": [3] }));
    }
    quiet(&mut stranger).await;
}

#[tokio::test]
async fn a_details_edit_carries_the_playlist_and_no_ops() {
    let (_temp, app, address) = world().await;
    let id = create(&app, json!({ "title": "Mix" })).await["id"].clone();
    let mut phone = connect_with(address, PHONE).await;
    edit(&app, id.as_str().unwrap(), json!({ "baseVersion": 1, "title": "Renamed" })).await;
    let message = change(&mut phone).await;
    assert_eq!(message["version"], json!(2));
    assert_eq!(message["playlist"]["title"], json!("Renamed"));
    assert_eq!(message.get("ops"), None);
    assert_eq!(message.get("refetch"), None);
}

#[tokio::test]
async fn an_edit_too_big_to_send_says_to_read_the_items_again() {
    let (_temp, app, address) = world().await;
    let id = create(&app, json!({ "title": "Mix" })).await["id"].clone();
    let mut phone = connect_with(address, PHONE).await;
    let items: Vec<Value> = (0..12000).map(|_| track(1)).collect();
    let body = json!({ "baseVersion": 1, "ops": [{ "op": "add", "items": items }] });
    let (status, edited) = edit(&app, id.as_str().unwrap(), body).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    let message = change(&mut phone).await;
    assert_eq!(message["version"], json!(2));
    assert_eq!(message["playlist"]["trackCount"], json!(12000));
    assert_eq!(message["refetch"], json!(true));
    assert_eq!(message.get("ops"), None);
}

#[tokio::test]
async fn creating_and_deleting_are_announced_to_the_owner_alone() {
    let (_temp, app, address) = world().await;
    let mut phone = connect_with(address, PHONE).await;
    let mut stranger = connect_with(address, STRANGER).await;

    let created = create(&app, json!({ "title": "Mix", "items": [track(1)] })).await;
    let message = change(&mut phone).await;
    assert_eq!(message["playlist"], created);
    assert_eq!((&message["version"], &message["playlistId"]), (&json!(1), &created["id"]));
    assert_eq!(message.get("ops"), None);

    let uri = format!("{URI}/{}", created["id"].as_str().unwrap());
    let (status, _) = call(&app, TOKEN, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let message = change(&mut phone).await;
    assert_eq!(message["playlist"], Value::Null, "{message}");
    assert_eq!(message["playlistId"], created["id"]);
    assert_eq!(message["libraryId"], json!("1"));
    assert_eq!(message["version"], json!(1));
    quiet(&mut stranger).await;
}

#[tokio::test]
async fn refused_requests_and_replays_send_nothing() {
    let (_temp, app, address) = world().await;
    let id = create(&app, json!({ "title": "Mix", "items": [track(1)] })).await["id"].clone();
    let id = id.as_str().unwrap();
    let mut phone = connect_with(address, PHONE).await;
    let uri = format!("{URI}/{id}");
    let add = json!([{ "op": "add", "items": [track(2)] }]);

    let (status, _) = edit(&app, id, json!({ "baseVersion": 7, "ops": add })).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let out_of_range = json!([{ "op": "remove", "positions": [9] }]);
    let (status, _) = edit(&app, id, json!({ "baseVersion": 1, "ops": out_of_range })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = call(&app, TOKEN, "DELETE", &format!("{URI}/999"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let body = json!({ "title": "Again", "items": [track(3)] });
    let (status, _) = call(&app, TOKEN, "POST", URI, Some(body)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let body = json!({ "baseVersion": 1, "ops": add });
    let (status, _) = send(&app, TOKEN, "PATCH", &uri, Some("k"), body.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, TOKEN, "PATCH", &uri, Some("k"), body).await;
    assert_eq!(status, StatusCode::OK);
    let new = json!({ "title": "Keyed" });
    send(&app, TOKEN, "POST", URI, Some("c"), new.clone()).await;
    send(&app, TOKEN, "POST", URI, Some("c"), new).await;

    // Only the two that applied were announced: the edit, then the keyed create.
    let first = change(&mut phone).await;
    assert_eq!((&first["playlistId"], &first["version"]), (&json!(id), &json!(2)));
    let second = change(&mut phone).await;
    assert_eq!(second["playlist"]["title"], json!("Keyed"));
    quiet(&mut phone).await;
}

#[tokio::test]
async fn an_edit_the_client_gave_up_on_is_still_announced_once_it_commits() {
    let (temp, db, app) = catalog().await;
    let conn = conn(&temp);
    add_device(&conn, 1, 2, PHONE);
    let address = serve(app.clone()).await;
    let id = create(&app, json!({ "title": "Mix", "items": [track(1)] })).await["id"].clone();
    let mut phone = connect_with(address, PHONE).await;

    // The writer is busy, as behind a scan, so the edit waits for it.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let busy = tokio::spawn({
        let db = db.clone();
        async move {
            db.write(move |_| {
                held.recv().unwrap();
                Ok(())
            })
            .await
            .unwrap();
        }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let body = json!({ "baseVersion": 1, "title": "Renamed" });
    let gave_up =
        tokio::time::timeout(Duration::from_millis(300), edit(&app, id.as_str().unwrap(), body))
            .await;
    assert!(gave_up.is_err(), "the writer was not busy");
    release.send(()).unwrap();
    busy.await.unwrap();

    let message = change(&mut phone).await;
    assert_eq!(message["version"], json!(2));
    assert_eq!(message["playlist"]["title"], json!("Renamed"));
}
