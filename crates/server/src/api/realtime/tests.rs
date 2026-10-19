use std::collections::BTreeMap;

use axum::http::StatusCode;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::{Error, Message};

use super::super::session::COOKIE;
use super::super::testing::socket::{Socket, connect, connect_with, next, next_of, serve};
use super::super::testing::{
    TOKEN, add_device, add_user, app, app_before_setup, json, respond, send, send_json,
};
use super::{Hub, ServerMessage};

mod revoked;

const PASSWORD: &str = "correct horse battery staple";

/// The next message other than the state every connection of the account is sent on
/// connecting and whenever it changes: its device list and its playback state.
async fn receive(socket: &mut Socket) -> Value {
    loop {
        let message = next(socket).await;
        if message["type"] != "devices" && message["type"] != "player" {
            return message;
        }
    }
}

/// The next device list, as whether each device is connected, by ID.
async fn devices(socket: &mut Socket) -> BTreeMap<String, bool> {
    let message = next_of(socket, "devices").await;
    let connected = |device: &Value| {
        // It would differ for each connection, so only `/me` responses say.
        assert_eq!(device.get("current"), None, "{device}");
        device["connected"].as_bool().unwrap()
    };
    let devices = message["devices"].as_array().unwrap().iter();
    devices.map(|device| (device["id"].as_str().unwrap().to_owned(), connected(device))).collect()
}

fn status(error: Error) -> StatusCode {
    match error {
        Error::Http(response) => response.status(),
        other => panic!("{other}"),
    }
}

#[tokio::test]
async fn a_device_connects_says_hello_and_answers_pings() {
    let (_temp, _db, app) = app("");
    let address = serve(app.clone()).await;
    let authorized = [("authorization", &*format!("Bearer {TOKEN}"))];
    let mut socket = connect(address, &authorized).await.unwrap();

    let hello = receive(&mut socket).await;
    assert_eq!((&hello["type"], &hello["deviceId"]), (&json!("hello"), &json!("1")));
    assert!(hello["serverTime"].as_str().unwrap().ends_with('Z'), "{hello}");
    let (_, devices) = json(&app, "/api/v1/me/devices").await;
    assert_eq!(devices["items"][0]["connected"], json!(true));

    socket.send(Message::text(r#"{"type":"notBuiltYet"}"#)).await.unwrap();
    socket.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    assert_eq!(receive(&mut socket).await["type"], json!("pong"), "unknown messages are ignored");

    socket.close(None).await.unwrap();
    while socket.next().await.is_some() {}
    // The server lets go of the connection just after it closes.
    for _ in 0..100 {
        let (_, devices) = json(&app, "/api/v1/me/devices").await;
        if devices["items"][0]["connected"] == json!(false) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("still connected after closing");
}

#[tokio::test]
async fn a_connection_needs_an_active_accounts_token() {
    let (temp, _db, app) = app("");
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "suspended");
    conn.execute("UPDATE users SET status = 'suspended' WHERE id = 2", []).unwrap();
    let address = serve(app).await;
    let refused = connect(address, &[]).await.unwrap_err();
    assert_eq!(status(refused), StatusCode::UNAUTHORIZED);
    for token in ["wrong", "suspended"] {
        let bearer = format!("Bearer {token}");
        let refused = connect(address, &[("authorization", &bearer)]).await.unwrap_err();
        assert_eq!(status(refused), StatusCode::UNAUTHORIZED, "{token}");
    }
}

#[tokio::test]
async fn a_page_connects_only_from_this_server() {
    let (_temp, _db, app) = app("");
    let address = serve(app).await;
    let cookie = format!("{COOKIE}={TOKEN}");

    let here = format!("http://{address}");
    assert!(connect(address, &[("cookie", &cookie), ("origin", &here)]).await.is_ok());
    // Behind a proxy that changes `Host`, or names or leaves out a default port.
    for (origin, header, host) in [
        ("https://music.example", "x-forwarded-host", "music.example"),
        ("https://music.example", "host", "music.example:443"),
        ("https://music.example:443", "host", "music.example"),
        ("https://Music.Example:8443", "host", "music.example:8443"),
        ("http://[::1]:8443", "host", "[::1]:8443"),
    ] {
        let proxied = [("cookie", &*cookie), ("origin", origin), (header, host)];
        assert!(connect(address, &proxied).await.is_ok(), "{origin} {header}: {host}");
    }

    let other_port = format!("http://{}:1", address.ip());
    for origin in ["https://evil.example", "null", &other_port] {
        let refused =
            connect(address, &[("cookie", &cookie), ("origin", origin)]).await.unwrap_err();
        assert_eq!(status(refused), StatusCode::FORBIDDEN, "{origin}");
    }
    for (origin, host) in [
        ("https://music.example:9000", "music.example:8443"),
        ("https://music.example", "music.example:8443"),
        // Another server on the same host: the same site, so sent the cookie.
        ("https://music.example:8443", "music.example"),
        ("http://music.example:9999", "music.example"),
        ("http://music.example", "music.example:443"),
        ("https://evil.example:8443", "music.example:8443"),
        ("http://[::2]:8443", "[::1]:8443"),
    ] {
        let proxied = [("cookie", &*cookie), ("origin", origin), ("x-forwarded-host", host)];
        let refused = connect(address, &proxied).await.unwrap_err();
        assert_eq!(status(refused), StatusCode::FORBIDDEN, "{origin} {host}");
    }
}

#[tokio::test]
async fn logging_out_closes_the_devices_connections_and_no_others() {
    let (temp, _db, app) = app("");
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_device(&conn, 1, 2, "other-token");
    let address = serve(app.clone()).await;
    let mut tabs = Vec::new();
    for _ in 0..2 {
        tabs.push(connect_with(address, TOKEN).await);
    }
    let mut other = connect_with(address, "other-token").await;

    let (status, _, _) = send(app, "POST", "/api/v1/auth/logout").await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    for mut tab in tabs {
        let revoked = receive(&mut tab).await;
        assert_eq!(revoked, json!({ "type": "sessionRevoked", "reason": "logged_out" }));
        assert!(matches!(tab.next().await, Some(Ok(Message::Close(_)))));
    }
    other.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    assert_eq!(receive(&mut other).await["type"], json!("pong"), "another device stays");
}

#[tokio::test]
async fn the_accounts_connections_see_its_devices_come_and_go() {
    let (temp, _db, app) = app("");
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_device(&conn, 1, 2, "phone");
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 3, "stranger");
    let address = serve(app.clone()).await;
    let mut desk = connect_with(address, TOKEN).await;
    // A list from before the latest change may arrive first; each must be only this account's.
    let until = async |socket: &mut Socket, expected: BTreeMap<String, bool>| loop {
        let list = devices(socket).await;
        assert!(list.keys().all(|id| id == "1" || id == "2"), "{list:?}");
        if list == expected {
            return;
        }
    };
    let both = |desk, phone| BTreeMap::from([("1".into(), desk), ("2".into(), phone)]);
    until(&mut desk, both(true, false)).await;

    let mut phone = connect_with(address, "phone").await;
    until(&mut desk, both(true, true)).await;
    until(&mut phone, both(true, true)).await;
    let mut stranger = connect_with(address, "stranger").await;
    assert_eq!(devices(&mut stranger).await, BTreeMap::from([("3".into(), true)]));

    phone.close(None).await.unwrap();
    until(&mut desk, both(true, false)).await;

    let logout = axum::http::Request::post("/api/v1/auth/logout")
        .header("authorization", "Bearer phone")
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(respond(app, logout).await.0, StatusCode::NO_CONTENT);
    until(&mut desk, BTreeMap::from([("1".into(), true)])).await;

    stranger.send(Message::text(r#"{"type":"ping"}"#)).await.unwrap();
    assert_eq!(next(&mut stranger).await["type"], json!("pong"), "no other account's lists");
}

#[tokio::test]
async fn logging_in_adds_a_device_to_the_list() {
    let (_temp, _db, app) = app_before_setup("");
    let device = json!({ "name": "Laptop", "type": "desktop" });
    let setup = json!({ "username": "sam", "password": PASSWORD, "device": device });
    let (_, _, session) = send_json(app.clone(), "POST", "/api/v1/setup", &setup).await;
    let laptop = session["device"]["id"].as_str().unwrap().to_owned();
    let address = serve(app.clone()).await;
    let mut socket = connect_with(address, session["token"].as_str().unwrap()).await;
    assert_eq!(devices(&mut socket).await, BTreeMap::from([(laptop.clone(), true)]));

    let login = json!({ "username": "sam", "password": PASSWORD, "device": device });
    let (status, _, session) = send_json(app, "POST", "/api/v1/auth/login", &login).await;
    assert_eq!(status, StatusCode::OK);
    let phone = session["device"]["id"].as_str().unwrap().to_owned();
    let expected = BTreeMap::from([(laptop, true), (phone, false)]);
    assert_eq!(devices(&mut socket).await, expected);
}

#[tokio::test]
async fn a_client_that_falls_behind_is_closed_once_it_has_what_was_queued() {
    let hub = std::sync::Arc::new(Hub::default());
    let mut slow = hub.connect(1, 1);
    let pong = ServerMessage::Pong { server_time: String::new() };
    for _ in 0..=super::hub::OUTBOX {
        hub.broadcast(1, &pong);
    }
    assert_eq!(hub.connected(1), Vec::<i64>::new());
    for _ in 0..super::hub::OUTBOX {
        assert!(slow.next().await.is_some());
    }
    assert!(slow.next().await.is_none(), "then closed");
}

#[tokio::test]
async fn a_device_list_waiting_its_turn_stands_for_later_changes() {
    /// Whether `turn` is still waiting once polled: it is polled before the timeout is.
    async fn pending(turn: impl Future) -> bool {
        tokio::time::timeout(std::time::Duration::ZERO, turn).await.is_err()
    }
    let hub = Hub::default();
    let sending = hub.device_list_turn(1).await.unwrap();
    let mut waiting = Box::pin(hub.device_list_turn(1));
    assert!(pending(&mut waiting).await);
    assert!(hub.device_list_turn(1).await.is_none(), "the waiting list is read after this");
    assert!(hub.device_list_turn(2).await.is_some(), "another account's is not");

    drop(sending);
    let reading = waiting.await.unwrap();
    let mut next = Box::pin(hub.device_list_turn(1));
    assert!(pending(&mut next).await, "the list being read may be stale");
    drop(reading);
    assert!(next.await.is_some());
}
