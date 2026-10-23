//! Helpers for tests of the realtime connection: a server on a port of its own, and sockets
//! to it.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Error;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Serves `app` on a port of its own, and returns its address.
pub async fn serve(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(crate::serve::serve(listener, app, std::future::pending(), async {}));
    address
}

/// Opens `/realtime` at `address` with `headers`.
pub async fn connect(
    address: SocketAddr,
    headers: &[(&'static str, &str)],
) -> Result<Socket, Error> {
    let mut request = format!("ws://{address}/api/v1/realtime").into_client_request().unwrap();
    for &(name, value) in headers {
        request.headers_mut().insert(name, value.parse().unwrap());
    }
    Ok(tokio_tungstenite::connect_async(request).await?.0)
}

/// Opens `/realtime` at `address` logged in with `token`, once the server has said hello: only
/// then does it count the connection.
pub async fn connect_with(address: SocketAddr, token: &str) -> Socket {
    let mut socket =
        connect(address, &[("authorization", &format!("Bearer {token}"))]).await.unwrap();
    next_of(&mut socket, "hello").await;
    socket
}

/// The next message, of any type. Panics if none arrives within 5 seconds.
pub async fn next(socket: &mut Socket) -> Value {
    let message = timeout(Duration::from_secs(5), socket.next()).await.expect("a message");
    serde_json::from_str(message.unwrap().unwrap().to_text().unwrap()).unwrap()
}

/// The next message of type `kind`, skipping any others.
pub async fn next_of(socket: &mut Socket, kind: &str) -> Value {
    loop {
        let message = next(socket).await;
        if message["type"] == kind {
            return message;
        }
    }
}
