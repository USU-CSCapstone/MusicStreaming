//! Some of these run on paused time: whenever every task waits, the clock jumps to the next
//! timer.

use std::convert::Infallible;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::ConnectInfo;
use axum::http::header::REFERRER_POLICY;
use axum::routing::{get, post};
use futures_util::stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout};
use tower_http::services::ServeDir;

use super::{BODY_READ_TIMEOUT, DRAIN_TIMEOUT, HEADER_READ_TIMEOUT, serve};

/// A request line and one header field, without the blank line that would end the header.
const PART_OF_A_HEADER: &[u8] = b"GET / HTTP/1.1\r\nHost: jewelcase\r\n";

/// Routes where `/` answers with the client's address and `/echo` with the body it was sent.
fn routes() -> Router {
    Router::new()
        .route(
            "/",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.to_string() }),
        )
        .route("/echo", post(|body: String| async move { body }))
}

/// Serves `app`, and returns its address, a way to stop it, and its task.
async fn start(app: Router) -> (SocketAddr, Arc<Notify>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stop = Arc::new(Notify::new());
    let stopped = stop.clone();
    let server =
        tokio::spawn(serve(listener, app, async move { stopped.notified().await }, async {}));
    (address, stop, server)
}

/// Connects to `address` and sends `request`.
async fn send(address: SocketAddr, request: &[u8]) -> TcpStream {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(request).await.unwrap();
    stream
}

/// Everything the server sends on `stream`, which it must close within a second: sooner than
/// any of its timeouts. The request must ask for `Connection: close`.
async fn response(mut stream: TcpStream) -> String {
    let mut response = String::new();
    timeout(Duration::from_secs(1), stream.read_to_string(&mut response)).await.unwrap().unwrap();
    response
}

/// Waits for the server to close `stream`, sending nothing more, and fails if it stays open
/// for twice the header timeout.
async fn closed(mut stream: TcpStream) {
    let mut rest = Vec::new();
    timeout(HEADER_READ_TIMEOUT * 2, stream.read_to_end(&mut rest)).await.unwrap().unwrap();
    assert_eq!(String::from_utf8_lossy(&rest), "");
}

// On real time: paused time can pass both timeouts before the kernel hands the server the
// connection, or the part of a header that was sent.
#[tokio::test]
async fn a_connection_that_sends_part_of_a_header_is_closed_after_the_timeout() {
    let (address, _stop, _server) = start(routes()).await;
    let opened = Instant::now();
    closed(send(address, PART_OF_A_HEADER).await).await;
    assert!(opened.elapsed() >= HEADER_READ_TIMEOUT, "{:?}", opened.elapsed());
}

// On real time: paused time can pass both timeouts before the kernel hands the server the
// connection, or the part of a header that was sent.
#[tokio::test]
async fn a_connection_that_sends_nothing_is_closed_after_the_timeout() {
    let (address, _stop, _server) = start(routes()).await;
    let opened = Instant::now();
    closed(send(address, b"").await).await;
    assert!(opened.elapsed() >= HEADER_READ_TIMEOUT, "{:?}", opened.elapsed());
}

// On real time: paused time can pass the 1 s limit on each read before the kernel hands the
// server the request.
#[tokio::test]
async fn a_connection_serves_a_second_request_after_its_response() {
    let (address, _stop, _server) = start(routes()).await;
    let request = b"GET / HTTP/1.1\r\nHost: jewelcase\r\n\r\n";
    let mut stream = send(address, request).await;
    let client = stream.local_addr().unwrap().to_string();
    for _ in 0..2 {
        // Each body is the client's address, so a response is whole once it ends with it.
        let mut response = Vec::new();
        while !response.ends_with(client.as_bytes()) {
            let mut buffer = [0; 512];
            let read = timeout(Duration::from_secs(1), stream.read(&mut buffer));
            let read = read.await.unwrap().unwrap();
            assert_ne!(read, 0, "closed: {}", String::from_utf8_lossy(&response));
            response.extend_from_slice(&buffer[..read]);
        }
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
        stream.write_all(request).await.unwrap();
    }
}

// On real time, as the other header timeout tests are.
#[tokio::test]
async fn an_idle_connection_stays_open_for_a_while_and_is_closed_by_the_timeout() {
    let (address, _stop, _server) = start(routes()).await;
    let mut stream = send(address, b"GET / HTTP/1.1\r\nHost: jewelcase\r\n\r\n").await;
    let mut buffer = [0; 512];
    assert_ne!(stream.read(&mut buffer).await.unwrap(), 0);
    // Still open: the read waits out its limit instead of ending.
    let idle = timeout(Duration::from_secs(1), stream.read(&mut buffer)).await;
    assert!(idle.is_err(), "closed or sent more: {idle:?}");
    let idle_since = Instant::now();
    closed(stream).await;
    assert!(idle_since.elapsed() <= HEADER_READ_TIMEOUT + Duration::from_secs(1));
}

// On real time: paused time can pass the 1 s limit on the read before the kernel hands the
// server the request.
#[tokio::test]
async fn handlers_are_told_the_client_address() {
    let (address, _stop, _server) = start(routes()).await;
    let request = b"GET / HTTP/1.1\r\nHost: jewelcase\r\nConnection: close\r\n\r\n";
    let stream = send(address, request).await;
    let client = stream.local_addr().unwrap();
    let response = response(stream).await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with(&format!("\r\n\r\n{client}")), "{response}");
}

// On real time: paused time could pass the 1 s limit while a file is read.
#[tokio::test]
async fn every_response_says_how_a_browser_may_use_it() {
    let web = tempfile::tempdir().unwrap();
    std::fs::write(web.path().join("robots.txt"), "robots").unwrap();
    let app = routes()
        .route("/own", get(|| async { ([(REFERRER_POLICY, "no-referrer")], "own") }))
        .fallback_service(ServeDir::new(web.path()));
    let (address, _stop, _server) = start(app).await;
    let get = |path: &str| {
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: jewelcase\r\nConnection: close\r\n\r\n");
        async move { response(send(address, request.as_bytes()).await).await }
    };
    // A handler's answer, a file's, and a file's that is not there.
    for (path, status) in
        [("/", "200 OK"), ("/robots.txt", "200 OK"), ("/missing", "404 Not Found")]
    {
        let response = get(path).await;
        assert!(response.starts_with(&format!("HTTP/1.1 {status}\r\n")), "{response}");
        for header in [
            "x-content-type-options: nosniff",
            "content-security-policy: frame-ancestors 'self'",
            "referrer-policy: same-origin",
        ] {
            assert!(response.contains(&format!("\r\n{header}\r\n")), "{path}: {response}");
        }
    }
    // A handler may set its own.
    let response = get("/own").await;
    assert!(response.contains("\r\nreferrer-policy: no-referrer\r\n"), "{response}");
    assert!(!response.contains("same-origin"), "{response}");
}

// On real time: paused time can jump past the 1 s limit while the kernel moves a megabyte.
#[tokio::test]
async fn a_streamed_response_arrives_in_full() {
    let chunks =
        || stream::iter((0..64).map(|_| Ok::<_, Infallible>(Bytes::from(vec![7; 16_384]))));
    let app = Router::new().route("/", get(move || async move { Body::from_stream(chunks()) }));
    let (address, _stop, _server) = start(app).await;
    let request = b"GET / HTTP/1.1\r\nHost: jewelcase\r\nConnection: close\r\n\r\n";
    let mut stream = send(address, request).await;
    let mut response = Vec::new();
    timeout(Duration::from_secs(1), stream.read_to_end(&mut response)).await.unwrap().unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    // The chunked framing is text, so every byte 7 is the body's.
    assert_eq!(response.iter().filter(|&&byte| byte == 7).count(), 64 * 16_384);
}

#[tokio::test(start_paused = true)]
async fn shutdown_closes_a_connection_that_has_sent_nothing_at_once() {
    let (address, stop, server) = start(routes()).await;
    let stream = send(address, b"").await;
    // Lets the server accept it.
    sleep(Duration::from_secs(1)).await;
    stop.notify_one();
    timeout(Duration::from_secs(1), server).await.unwrap().unwrap();
    closed(stream).await;
}

// On real time: paused time can pass the header timeout before the kernel hands the server
// the part of a header that was sent.
#[tokio::test]
async fn shutdown_waits_for_part_of_a_header_only_until_the_timeout() {
    let (address, stop, server) = start(routes()).await;
    let opened = Instant::now();
    let stream = send(address, PART_OF_A_HEADER).await;
    sleep(Duration::from_millis(100)).await;
    stop.notify_one();
    timeout(HEADER_READ_TIMEOUT, server).await.unwrap().unwrap();
    assert!(opened.elapsed() <= HEADER_READ_TIMEOUT, "{:?}", opened.elapsed());
    closed(stream).await;
}

#[tokio::test(start_paused = true)]
async fn a_body_that_stops_arriving_times_out() {
    let (address, _stop, _server) = start(routes()).await;
    let request = b"POST /echo HTTP/1.1\r\nHost: jewelcase\r\nContent-Length: 10\r\n\r\nabc";
    let sent = Instant::now();
    let mut stream = send(address, request).await;
    let mut response = String::new();
    let read = stream.read_to_string(&mut response);
    timeout(BODY_READ_TIMEOUT * 2, read).await.unwrap().unwrap();
    assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{response}");
    assert!(sent.elapsed() >= BODY_READ_TIMEOUT, "{:?}", sent.elapsed());
}

#[tokio::test(start_paused = true)]
async fn shutdown_waits_for_a_trickling_body_only_until_the_drain_timeout() {
    let (address, stop, server) = start(routes()).await;
    let request = b"POST /echo HTTP/1.1\r\nHost: jewelcase\r\nContent-Length: 1000\r\n\r\n";
    let mut stream = send(address, request).await;
    // One byte every few seconds: never long enough apart for the body to time out.
    tokio::spawn(async move {
        while stream.write_all(b"a").await.is_ok() {
            sleep(Duration::from_secs(3)).await;
        }
    });
    sleep(Duration::from_secs(1)).await;
    let stopping = Instant::now();
    stop.notify_one();
    timeout(DRAIN_TIMEOUT + Duration::from_secs(1), server).await.unwrap().unwrap();
    assert!(stopping.elapsed() >= DRAIN_TIMEOUT, "{:?}", stopping.elapsed());
}

#[tokio::test(start_paused = true)]
async fn shutdown_waits_for_a_request_in_progress() {
    let answer = Arc::new(Notify::new());
    let answered = answer.clone();
    let app = Router::new().route(
        "/",
        get(move || async move {
            answered.notified().await;
            "done"
        }),
    );
    let (address, stop, mut server) = start(app).await;
    let request = b"GET / HTTP/1.1\r\nHost: jewelcase\r\nConnection: close\r\n\r\n";
    let stream = send(address, request).await;
    sleep(Duration::from_secs(1)).await;
    stop.notify_one();
    timeout(DRAIN_TIMEOUT - Duration::from_secs(1), &mut server).await.unwrap_err();
    answer.notify_one();
    timeout(Duration::from_secs(1), server).await.unwrap().unwrap();
    let response = response(stream).await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("\r\n\r\ndone"), "{response}");
}

/// The listener closes as the server stops, before `as_it_stops` runs, so a connection made
/// while it runs is refused at once rather than served or left waiting.
#[tokio::test]
async fn a_connection_made_as_the_server_stops_is_refused() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let (began, beginning) = oneshot::channel::<()>();
    let (finish, finishing) = oneshot::channel::<()>();
    let as_it_stops = async move {
        began.send(()).unwrap();
        finishing.await.unwrap();
    };
    let shutdown = async move { stopped.await.unwrap() };
    let server = tokio::spawn(serve(listener, routes(), shutdown, as_it_stops));
    stop.send(()).unwrap();
    beginning.await.unwrap();
    let connected = timeout(Duration::from_millis(500), TcpStream::connect(address)).await;
    let refused = connected.unwrap().unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::ConnectionRefused);
    finish.send(()).unwrap();
    timeout(Duration::from_secs(1), server).await.unwrap().unwrap();
}
