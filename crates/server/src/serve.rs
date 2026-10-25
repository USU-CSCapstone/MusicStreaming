//! Serves the app over HTTP/1, keeping a connection open for the client's next request. A client
//! that sends nothing, between requests too, or stops partway through a request's header or
//! body, loses its connection after a timeout. One that keeps a body trickling in keeps its
//! connection, and a file descriptor with it, until a route's own limits end it: the routes open
//! before sign-in allow a body 30 s in all (`api`). Nothing caps how many connections are open
//! at once. The shutdown waits for requests in progress only up to a deadline.
//!
//! Every response tells a browser to take it as the type it says, to show it framed only by
//! the server's own pages, and to tell other sites nothing of the page that linked to them.

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::pin::pin;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use axum::http::header::{CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderName, HeaderValue};
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

/// How long a client has to send a request's header once its connection opens or its last
/// response is sent, so it is also how long an idle connection stays open.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// The longest a request's body may go without a byte arriving while its handler reads it. It
/// allows for a slow or briefly lost link, and a slow but steady upload keeps going.
const BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the shutdown waits for the requests in progress. An API request takes milliseconds,
/// so one still running after this is a stream or an upload, which its client resumes. It is a
/// third of the 10 s `docker stop` allows before it kills the server. The save runs alongside
/// it, one write that may wait behind a scan's batch, and the rest is left for the background
/// stop after it, which waits for each scanner's file in hand.
pub(crate) const DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Serves `app` on `listener` until `shutdown` resolves. Then it closes the listener, so a new
/// connection is refused, closes connections that have no request yet, and runs `as_it_stops`
/// while it waits for the requests in progress, counting a header or body still arriving. It
/// returns once `as_it_stops` is done and the requests are answered or [`DRAIN_TIMEOUT`] has
/// passed, leaving any still open to the runtime's end.
///
/// A connection upgraded to a WebSocket belongs to its handler, so it is not waited for.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()>,
    as_it_stops: impl Future<Output = ()>,
) {
    // Each is set only on a response whose handler has not set its own.
    let header = |name: HeaderName, value: &'static str| {
        SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
    };
    let app = app.layer(RequestBodyTimeoutLayer::new(BODY_READ_TIMEOUT)).layer((
        header(X_CONTENT_TYPE_OPTIONS, "nosniff"),
        header(CONTENT_SECURITY_POLICY, "frame-ancestors 'self'"),
        header(REFERRER_POLICY, "same-origin"),
    ));
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
    drop(listener);
    // Each connection holds a receiver until it ends.
    drop(stopping);
    stop.send_replace(());
    let (drained, ()) = tokio::join!(timeout(DRAIN_TIMEOUT, stop.closed()), as_it_stops);
    if drained.is_err() {
        warn!("stopping with requests still in progress after {DRAIN_TIMEOUT:?}");
    }
}

#[cfg(test)]
mod tests;
