//! The `http` import against a server on 127.0.0.1: requests and redirects, the addresses they
//! reach, the shared cache, and rate limits, with what they share.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::super::Run;
use super::super::testing::{self, Memory};
use super::http::{self, Host as _};
use crate::manifest::Permission;
use crate::network::Network;

mod addresses;
mod cache;
mod clients;
mod limits;
mod requests;

/// How many requests the test server has had for each path.
type Hits = Arc<Mutex<HashMap<String, usize>>>;

/// A server on 127.0.0.1, the port it listens on, and its hits. It answers:
///
/// - `/away` with a redirect to `localhost`, `/near` with one to `/here` on itself, `/far`
///   with one to `/fresh` on `localhost`, `/to-ip` with one to `/fresh` on `127.0.0.1`, and
///   `/loop` with one to `[::1]`;
/// - `/fresh` with how many times it was asked, fresh for a minute;
/// - `/checked` with how many times it was asked and an ETag, to be checked every time, and
///   `304 Not Modified` to a check;
/// - `/moved` with a redirect to `/checked` on `localhost`, and `304 Not Modified` to a check;
/// - `/busy` with `429 Too Many Requests` for two minutes, and `/sent-on` with a redirect to
///   `/busy` on `localhost`;
/// - `/hops/n` with a redirect to `/hops/n-1`, and `/hops/0` with `hello`;
/// - `/down` with `500 Internal Server Error`, and `/none` with `404 Not Found`;
/// - `/agent` with the `User-Agent` it was sent, lowercase;
/// - `/slow` with nothing, holding the connection open;
/// - anything else with `hello`.
async fn server() -> (u16, Hits) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Hits::default();
    let counted = hits.clone();
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let read = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            let path = request.split(' ').nth(1).unwrap().to_owned();
            if path == "/slow" {
                held.push(socket);
                continue;
            }
            let hit = {
                let mut hits = counted.lock().unwrap();
                let hit = hits.entry(path.clone()).or_default();
                *hit += 1;
                *hit
            };
            let ok = |headers: &str, body: &str| {
                format!("HTTP/1.1 200 OK\r\n{headers}Content-Length: {}\r\n\r\n{body}", body.len())
            };
            let reply = match path.as_str() {
                "/away" => format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://localhost:{port}/\r\nContent-Length: 0\r\n\r\n"
                ),
                "/to-ip" => format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/fresh\r\nContent-Length: 0\r\n\r\n"
                ),
                "/loop" => format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://[::1]:{port}/\r\nContent-Length: 0\r\n\r\n"
                ),
                "/near" => {
                    "HTTP/1.1 302 Found\r\nLocation: /here\r\nContent-Length: 0\r\n\r\n".to_owned()
                }
                "/far" => format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://localhost:{port}/fresh\r\nContent-Length: 0\r\n\r\n"
                ),
                "/fresh" => ok("Cache-Control: max-age=60\r\n", &hit.to_string()),
                "/checked" if request.contains("if-none-match: \"v1\"") => {
                    "HTTP/1.1 304 Not Modified\r\nCache-Control: max-age=60\r\nContent-Length: 0\r\n\r\n"
                        .to_owned()
                }
                "/checked" => ok("Cache-Control: no-cache\r\nETag: \"v1\"\r\n", &hit.to_string()),
                "/moved" if request.contains("if-none-match: \"v1\"") => {
                    "HTTP/1.1 304 Not Modified\r\nCache-Control: max-age=60\r\nContent-Length: 0\r\n\r\n"
                        .to_owned()
                }
                "/moved" => format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://localhost:{port}/checked\r\nContent-Length: 0\r\n\r\n"
                ),
                "/sent-on" => format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://localhost:{port}/busy\r\nContent-Length: 0\r\n\r\n"
                ),
                "/busy" => "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Length: 0\r\n\r\n"
                    .to_owned(),
                "/down" => "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".to_owned(),
                "/none" => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_owned(),
                "/agent" => {
                    let agent = request.lines().find_map(|line| line.strip_prefix("user-agent: "));
                    ok("", agent.unwrap_or_default())
                }
                hops if hops.starts_with("/hops/") => match hops["/hops/".len()..].parse::<u32>().unwrap() {
                    0 => ok("", "hello"),
                    n => format!(
                        "HTTP/1.1 302 Found\r\nLocation: /hops/{}\r\nContent-Length: 0\r\n\r\n",
                        n - 1
                    ),
                },
                _ => ok("X-Served: yes\r\n", "hello"),
            };
            socket.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    (port, hits)
}

/// A run of plugin `test` in library 1, granted the network to 127.0.0.1 only.
fn run() -> Run<Memory> {
    testing::run(Memory::default(), &[Permission::Network])
}

/// A run of `plugin` in `library`, granted the network to 127.0.0.1 only, sharing `network`.
fn with(network: Arc<Network>, plugin: &str, library: i64) -> Run<Memory> {
    let mut run = run();
    run.network = network;
    run.grants.plugin = plugin.into();
    run.grants.library = library;
    run
}

/// A GET of `url`, with no headers or body.
fn get(url: String) -> http::Request {
    http::Request { method: http::Method::Get, url, headers: Vec::new(), body: Vec::new() }
}

/// The body of a GET of `path` on the test server, as text.
async fn body(run: &mut Run<Memory>, port: u16, path: &str) -> String {
    let reply = run.send(get(format!("http://127.0.0.1:{port}{path}"))).await.unwrap();
    assert_eq!(reply.status, 200);
    String::from_utf8(reply.body).unwrap()
}
