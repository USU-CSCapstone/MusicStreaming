//! Serves the app over HTTP/1, closing each connection after its response. A client that sends
//! nothing, or stops partway through a request's header or body, loses its connection, so it
//! cannot hold a file descriptor for long. The shutdown waits for requests in progress only up
//! to a deadline.

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::pin::pin;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use axum::http::HeaderValue;
use axum::http::header::CONNECTION;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::time::{sleep, timeout};
use tower_http::add_extension::AddExtension;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::RequestBodyTimeoutLayer;
use tracing::{debug, warn};

/// How long a client has to send a request's header once its connection opens.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The longest a request's body may go without a byte arriving while its handler reads it. It
/// allows for a slow or briefly lost link, and a slow but steady upload keeps going.
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the shutdown waits for the requests in progress: inside the 10 s `docker stop`
/// allows before it kills the server, with time left to save what the server holds.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(8);

/// Serves `app` on `listener` until `shutdown` resolves. Then it stops accepting, closes
/// connections that have no request yet, and returns once the requests in progress are
/// answered, counting a header or body still arriving, or after [`DRAIN_TIMEOUT`], leaving any
/// still open to the runtime's end.
///
/// Over HTTP/1.1, each connection closes after its response, so no connection sits idle where
/// a pooling proxy could send a request on it just as it closes. The exception is an HTTP/1.0
/// request that asks for keep-alive: hyper replaces the `close` with `Connection: keep-alive`,
/// so that connection stays open until [`HEADER_READ_TIMEOUT`] passes with no next request. A
/// connection upgraded to a WebSocket belongs to its handler, so it is not waited for.
pub async fn serve(listener: TcpListener, app: Router, shutdown: impl Future<Output = ()>) {
    // Not hyper's `keep_alive(false)`: that sets `Connection: close` on a WebSocket's `101`
    // too, which then fails to upgrade.
    let close = HeaderValue::from_static("close");
    let app = app
        .layer(RequestBodyTimeoutLayer::new(BODY_READ_TIMEOUT))
        .layer(SetResponseHeaderLayer::if_not_present(CONNECTION, close));
    let mut http = http1::Builder::new();
    http.timer(TokioTimer::new()).header_read_timeout(HEADER_READ_TIMEOUT);
    let (stop, stopping) = watch::channel(());
    let mut shutdown = pin!(shutdown);
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = &mut shutdown => break,
        };
        let (stream, peer) = match accepted {
            Ok(accepted) => accepted,
            // The client gave up before it was accepted.
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::ConnectionAborted
                        | ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionRefused
                ) =>
            {
                continue;
            }
            Err(error) => {
                // Usually out of file descriptors, so wait for some to close.
                warn!(%error, "cannot accept a connection");
                tokio::select! {
                    () = sleep(Duration::from_secs(1)) => continue,
                    () = &mut shutdown => break,
                }
            }
        };
        // Sign-in limits need the client's address (`api::origin`).
        let app = AddExtension::new(app.clone(), ConnectInfo::<SocketAddr>(peer));
        let connection = http
            .serve_connection(TokioIo::new(stream), TowerToHyperService::new(app))
            .with_upgrades();
        let mut stopping = stopping.clone();
        tokio::spawn(async move {
            let mut connection = pin!(connection);
            let result = tokio::select! {
                result = connection.as_mut() => result,
                // Told to stop, or `serve` itself was dropped: either way, finish up.
                _ = stopping.changed() => {
                    connection.as_mut().graceful_shutdown();
                    connection.await
                }
            };
            if let Err(error) = result {
                debug!(%error, "connection closed");
            }
        });
    }
    // Each connection holds a receiver until it ends.
    drop(stopping);
    stop.send_replace(());
    if timeout(DRAIN_TIMEOUT, stop.closed()).await.is_err() {
        warn!("stopping with requests still in progress after {DRAIN_TIMEOUT:?}");
    }
}

#[cfg(test)]
mod tests;
