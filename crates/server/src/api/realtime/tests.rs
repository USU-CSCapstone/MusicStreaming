use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use axum::http::StatusCode;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::super::session::COOKIE;
use super::super::testing::{
    TOKEN, add_device, add_user, app, app_before_setup, json, respond, send, send_json,
};
use super::{Hub, ServerMessage};

const PASSWORD: &str = "correct horse battery staple";

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Serves [`app`] on a port of its own, and returns its address.
async fn serve(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

/// Opens `/realtime` at `address` with `headers`.
async fn connect(address: SocketAddr, headers: &[(&'static str, &str)]) -> Result<Socket, Error> {
    let mut request = format!("ws://{address}/api/v1/realtime").into_client_request().unwrap();
    for &(name, value) in headers {
        request.headers_mut().insert(name, value.parse().unwrap());
    }
    Ok(tokio_tungstenite::connect_async(request).await?.0)
}

/// The next message, of any type.
async fn next(socket: &mut Socket) -> Value {
    let message = timeout(Duration::from_secs(5), socket.next()).await.expect("a message");
    serde_json::from_str(message.unwrap().unwrap().to_text().unwrap()).unwrap()
}

/// The next message other than a device list, which arrives whenever a device of the account
/// connects, disconnects, logs in, or logs out.
async fn receive(socket: &mut Socket) -> Value {
    loop {
        let message = next(socket).await;
        if message["type"] != "devices" {
            return message;
        }
    }
}

/// The next device list, as whether each device is connected, by ID.
async fn devices(socket: &mut Socket) -> BTreeMap<String, bool> {
    loop {
        let message = next(socket).await;
        if message["type"] == "devices" {
            let devices = message["devices"].as_array().unwrap().iter();
            let connected = |device: &Value| {
                // It would differ for each connection, so only `/me` responses say.
                assert_eq!(device.get("current"), None, "{device}");
                device["connected"].as_bool().unwrap()
            };
            return devices
                .map(|device| (device["id"].as_str().unwrap().to_owned(), connected(device)))
                .collect();
        }
    }
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
    let mut socket =
        connect(address, &[("authorization", &format!("Bearer {TOKEN}"))]).await.unwrap();

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
async fn a_connection_needs_a_token() {
    let (_temp, _db, app) = app("");
    let address = serve(app).await;
    let refused = connect(address, &[]).await.unwrap_err();
    assert_eq!(status(refused), StatusCode::UNAUTHORIZED);
    let refused = connect(address, &[("authorization", "Bearer wrong")]).await.unwrap_err();
    assert_eq!(status(refused), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_page_connects_only_from_this_server() {
    let (_temp, _db, app) = app("");
    let address = serve(app).await;
    let cookie = format!("{COOKIE}={TOKEN}");

    let here = format!("http://{address}");
    assert!(connect(address, &[("cookie", &cookie), ("origin", &here)]).await.is_ok());
    let proxied = [
        ("cookie", &*cookie),
        ("origin", "https://music.example"),
        ("x-forwarded-host", "music.example"),
    ];
    assert!(connect(address, &proxied).await.is_ok(), "behind a proxy that changes Host");

    for origin in ["https://evil.example", "null"] {
        let refused =
            connect(address, &[("cookie", &cookie), ("origin", origin)]).await.unwrap_err();
        assert_eq!(status(refused), StatusCode::FORBIDDEN, "{origin}");
    }
}

#[tokio::test]
async fn logging_out_closes_the_devices_connections_and_no_others() {
    let (temp, _db, app) = app("");
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_device(&conn, 1, 2, "other-token");
    let address = serve(app.clone()).await;
    let bearer = format!("Bearer {TOKEN}");
    let mut tabs = Vec::new();
    for _ in 0..2 {
        let mut tab = connect(address, &[("authorization", &bearer)]).await.unwrap();
        receive(&mut tab).await;
        tabs.push(tab);
    }
    let mut other = connect(address, &[("authorization", "Bearer other-token")]).await.unwrap();
    receive(&mut other).await;

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
    let mut desk =
        connect(address, &[("authorization", &format!("Bearer {TOKEN}"))]).await.unwrap();
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

    let mut phone = connect(address, &[("authorization", "Bearer phone")]).await.unwrap();
    until(&mut desk, both(true, true)).await;
    until(&mut phone, both(true, true)).await;
    let mut stranger = connect(address, &[("authorization", "Bearer stranger")]).await.unwrap();
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
    let bearer = format!("Bearer {}", session["token"].as_str().unwrap());
    let address = serve(app.clone()).await;
    let mut socket = connect(address, &[("authorization", &bearer)]).await.unwrap();
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
