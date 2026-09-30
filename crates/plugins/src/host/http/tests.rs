use std::time::Instant;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wasmtime::StoreLimitsBuilder;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::WasiCtx;

use super::super::{Grants, Refused, Run};
use super::http::{self, Host as _};
use crate::manifest::Permission;

/// A server on 127.0.0.1 that redirects `/away` to `localhost`, `/near` to itself, and answers
/// anything else with `hello`, and the port it listens on.
async fn server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let read = socket.read(&mut request).await.unwrap();
            let path =
                String::from_utf8_lossy(&request[..read]).split(' ').nth(1).unwrap().to_owned();
            let reply = match path.as_str() {
                "/away" => format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://localhost:{port}/\r\nContent-Length: 0\r\n\r\n"
                ),
                "/near" => {
                    "HTTP/1.1 302 Found\r\nLocation: /here\r\nContent-Length: 0\r\n\r\n".to_owned()
                }
                _ => {
                    "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-Served: yes\r\n\r\nhello".to_owned()
                }
            };
            socket.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    port
}

/// A run granted the network to 127.0.0.1 only.
fn run() -> Run<Refused> {
    Run {
        wasi: WasiCtx::builder().build(),
        table: ResourceTable::new(),
        limits: StoreLimitsBuilder::new().build(),
        deadline: Instant::now(),
        grants: Grants {
            permissions: vec![Permission::Network],
            destinations: vec!["127.0.0.1".into()],
        },
        library: Refused,
        client: None,
        roots: None,
        log: Vec::new(),
        written: 0,
        touched: Vec::new(),
    }
}

fn get(url: String) -> http::Request {
    http::Request { method: http::Method::Get, url, headers: Vec::new(), body: Vec::new() }
}

#[tokio::test]
async fn follows_redirects_only_to_approved_destinations() {
    let port = server().await;
    let mut run = run();
    let reply = run.send(get(format!("http://127.0.0.1:{port}/near"))).await.unwrap();
    assert_eq!((reply.status, reply.body.as_slice()), (200, b"hello".as_slice()));
    assert!(reply.headers.iter().any(|(name, value)| name == "x-served" && value == "yes"));

    let refused = run.send(get(format!("http://127.0.0.1:{port}/away"))).await.unwrap_err();
    assert!(refused.contains("redirected elsewhere: localhost is not one of"), "{refused}");
}

#[tokio::test]
async fn the_host_header_is_the_hosts() {
    let port = server().await;
    let mut request = get(format!("http://127.0.0.1:{port}/"));
    request.headers.push(("Host".into(), "elsewhere.example".into()));
    assert_eq!(run().send(request).await.unwrap_err(), "Host is set by the host");
}

#[tokio::test]
async fn needs_the_network_and_an_approved_destination() {
    let mut run = run();
    assert!(run.send(get("http://example.com/".into())).await.unwrap_err().contains("not one of"));
    run.grants.permissions.clear();
    assert_eq!(
        run.send(get("http://127.0.0.1/".into())).await.unwrap_err(),
        "Network access was not granted"
    );
}
