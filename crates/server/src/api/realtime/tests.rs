use std::net::SocketAddr;

use axum::http::StatusCode;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::super::session::COOKIE;
use super::super::testing::{TOKEN, app, json};

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

async fn receive(socket: &mut Socket) -> Value {
    let message = socket.next().await.unwrap().unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
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
