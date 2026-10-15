use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wasmtime::StoreLimitsBuilder;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::WasiCtx;

use super::super::{Grants, Refused, Run};
use super::http::{self, Error, Host as _};
use crate::manifest::{Per, Permission, RateLimit, rate_limit_gap};
use crate::network::Network;
use crate::network::limits::Scope;

/// How many requests the test server has had for each path.
type Hits = Arc<Mutex<HashMap<String, usize>>>;

/// A server on 127.0.0.1, the port it listens on, and its hits. It answers:
///
/// - `/away` with a redirect to `localhost`, `/near` with one to `/here` on itself, and `/far`
///   with one to `/fresh` on `localhost`;
/// - `/fresh` with how many times it was asked, fresh for a minute;
/// - `/checked` with how many times it was asked and an ETag, to be checked every time, and
///   `304 Not Modified` to a check;
/// - `/moved` with a redirect to `/checked` on `localhost`, and `304 Not Modified` to a check;
/// - `/busy` with `429 Too Many Requests` for two minutes, and `/sent-on` with a redirect to
///   `/busy` on `localhost`;
/// - anything else with `hello`.
async fn server() -> (u16, Hits) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Hits::default();
    let counted = hits.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 1024];
            let read = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            let path = request.split(' ').nth(1).unwrap().to_owned();
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
                _ => ok("X-Served: yes\r\n", "hello"),
            };
            socket.write_all(reply.as_bytes()).await.unwrap();
        }
    });
    (port, hits)
}

/// A run of plugin `test` in library 1, granted the network to 127.0.0.1 only.
fn run() -> Run<Refused> {
    with(Arc::default(), "test", 1)
}

/// A run of `plugin` in `library`, granted the network to 127.0.0.1 only, sharing `network`.
fn with(network: Arc<Network>, plugin: &str, library: i64) -> Run<Refused> {
    Run {
        wasi: WasiCtx::builder().build(),
        table: ResourceTable::new(),
        limits: StoreLimitsBuilder::new().build(),
        deadline: Instant::now(),
        grants: Grants {
            plugin: plugin.into(),
            library,
            user: None,
            permissions: vec![Permission::Network],
            destinations: vec!["127.0.0.1".into()],
            rate_limits: Vec::new(),
            host_limits: Vec::new(),
            settings: Default::default(),
        },
        library: Refused,
        client: None,
        network,
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
    let (port, _) = server().await;
    let mut run = run();
    let reply = run.send(get(format!("http://127.0.0.1:{port}/near"))).await.unwrap();
    assert_eq!((reply.status, reply.body.as_slice()), (200, b"hello".as_slice()));
    assert!(reply.headers.iter().any(|(name, value)| name == "x-served" && value == "yes"));

    let failed = run.send(get(format!("http://127.0.0.1:{port}/away"))).await.unwrap_err();
    assert!(
        matches!(&failed, Error::Failed(why) if why.contains("redirected elsewhere: localhost is not one of")),
        "{failed:?}"
    );
}

#[tokio::test]
async fn the_host_header_is_the_hosts() {
    let (port, _) = server().await;
    let mut request = get(format!("http://127.0.0.1:{port}/"));
    request.headers.push(("Host".into(), "elsewhere.example".into()));
    let refused = run().send(request).await.unwrap_err();
    assert!(
        matches!(&refused, Error::Refused(why) if why == "Host is set by the host"),
        "{refused:?}"
    );
}

#[tokio::test]
async fn needs_the_network_and_an_approved_destination() {
    let mut run = run();
    let refused = run.send(get("http://example.com/".into())).await.unwrap_err();
    assert!(matches!(&refused, Error::Refused(why) if why.contains("not one of")), "{refused:?}");
    run.grants.permissions.clear();
    let refused = run.send(get("http://127.0.0.1/".into())).await.unwrap_err();
    assert!(matches!(refused, Error::NotGranted), "{refused:?}");
}

/// The body of a GET of `path` on the test server, as text.
async fn body(run: &mut Run<Refused>, port: u16, path: &str) -> String {
    let reply = run.send(get(format!("http://127.0.0.1:{port}{path}"))).await.unwrap();
    assert_eq!(reply.status, 200);
    String::from_utf8(reply.body).unwrap()
}

#[tokio::test]
async fn a_fresh_reply_is_shared_within_a_library_and_never_beyond_it() {
    let (port, hits) = server().await;
    let network = Arc::new(Network::default());
    assert_eq!(body(&mut with(network.clone(), "artwork", 1), port, "/fresh").await, "1");
    // Another plugin in the same library asks the same question, and the service is not asked.
    assert_eq!(body(&mut with(network.clone(), "biographies", 1), port, "/fresh").await, "1");
    // One that can read the library does not share with those that cannot, who could learn
    // from what it asked what the library holds.
    let mut reader = with(network.clone(), "reader", 1);
    reader.grants.permissions.push(Permission::LibraryRead);
    assert_eq!(body(&mut reader, port, "/fresh").await, "2");
    // In another library it is asked again.
    assert_eq!(body(&mut with(network, "artwork", 2), port, "/fresh").await, "3");
    assert_eq!(hits.lock().unwrap()["/fresh"], 3);
}

#[tokio::test]
async fn a_change_that_succeeds_forgets_what_was_kept_for_its_url() {
    let (port, hits) = server().await;
    let mut run = run();
    assert_eq!(body(&mut run, port, "/fresh").await, "1");
    assert_eq!(body(&mut run, port, "/fresh").await, "1");
    let mut post = get(format!("http://127.0.0.1:{port}/fresh"));
    post.method = http::Method::Post;
    assert_eq!(run.send(post).await.unwrap().status, 200);
    assert_eq!(body(&mut run, port, "/fresh").await, "3");
    assert_eq!(hits.lock().unwrap()["/fresh"], 3);
}

#[tokio::test]
async fn a_stale_reply_is_checked_rather_than_fetched_again() {
    let (port, hits) = server().await;
    let mut run = run();
    assert_eq!(body(&mut run, port, "/checked").await, "1");
    // Checked with its ETag: the 304 is answered with the kept reply, and makes it fresh.
    assert_eq!(body(&mut run, port, "/checked").await, "1");
    assert_eq!(body(&mut run, port, "/checked").await, "1");
    assert_eq!(hits.lock().unwrap()["/checked"], 2);
}

#[tokio::test]
async fn a_kept_reply_is_not_handed_to_a_plugin_that_could_not_follow_its_redirect() {
    let (port, _) = server().await;
    let network = Arc::new(Network::default());
    let mut both = with(network.clone(), "both", 1);
    both.grants.destinations.push("localhost".into());
    assert_eq!(body(&mut both, port, "/far").await, "1");
    let mut near = with(network, "near", 1);
    let failed = near.send(get(format!("http://127.0.0.1:{port}/far"))).await.unwrap_err();
    assert!(
        matches!(&failed, Error::Failed(why) if why.contains("redirected elsewhere: localhost is not one of")),
        "{failed:?}"
    );
}

#[tokio::test]
async fn a_stale_reply_is_not_checked_for_a_plugin_that_could_not_follow_its_redirect() {
    let (port, _) = server().await;
    let network = Arc::new(Network::default());
    let mut both = with(network.clone(), "both", 1);
    both.grants.destinations.push("localhost".into());
    assert_eq!(body(&mut both, port, "/moved").await, "1");
    // A check would be answered `304` without the redirect, so it is asked afresh, and refused.
    let mut near = with(network, "near", 1);
    let failed = near.send(get(format!("http://127.0.0.1:{port}/moved"))).await.unwrap_err();
    assert!(
        matches!(&failed, Error::Failed(why) if why.contains("redirected elsewhere")),
        "{failed:?}"
    );
    // That failure left the reply kept, for a plugin that can still check it.
    assert_eq!(body(&mut both, port, "/moved").await, "1");
}

#[tokio::test]
async fn every_spelling_of_a_url_shares_what_was_kept_for_it() {
    let (port, hits) = server().await;
    let mut run = run();
    run.grants.destinations.push("localhost".into());
    for url in
        [format!("http://LOCALHOST:{port}/fresh"), format!("HTTP://localhost:{port}/./fresh")]
    {
        assert_eq!(run.send(get(url)).await.unwrap().body, b"1");
    }
    let mut post = get(format!("http://Localhost:{port}/fresh"));
    post.method = http::Method::Post;
    assert_eq!(run.send(post).await.unwrap().status, 200);
    let reply = run.send(get(format!("http://localhost:{port}/fresh"))).await.unwrap();
    assert_eq!(reply.body, b"3");
    assert_eq!(hits.lock().unwrap()["/fresh"], 3);
}

/// That `error` says to wait out the test server's two-minute `Retry-After`, less what has
/// passed since.
fn assert_waits_two_minutes(error: Error) {
    let Error::RateLimited(ms) = error else { panic!("{error:?}") };
    assert!((110_000..=120_000).contains(&ms), "{ms}");
}

/// A limit on requests to the test server, such as `"5/s"`.
fn limit(pace: &str, per: Per) -> RateLimit {
    let gap = rate_limit_gap(pace).unwrap();
    RateLimit { host: "127.0.0.1".into(), limit: pace.into(), gap, per }
}

/// The test server's host limit in force, a millisecond apart.
fn host_limit() -> (String, Duration) {
    ("127.0.0.1".into(), Duration::from_millis(1))
}

#[tokio::test]
async fn a_429_pauses_only_the_plugin_that_was_told() {
    let (port, _) = server().await;
    let network = Arc::new(Network::default());
    let mut told = with(network.clone(), "told", 1);
    let reply = told.send(get(format!("http://127.0.0.1:{port}/busy"))).await.unwrap();
    assert_eq!(reply.status, 429);
    let refused = told.send(get(format!("http://127.0.0.1:{port}/"))).await.unwrap_err();
    assert_waits_two_minutes(refused);
    assert_eq!(body(&mut with(network, "other", 1), port, "/").await, "hello");
}

#[tokio::test]
async fn a_429_for_one_user_pauses_only_their_runs() {
    let (port, _) = server().await;
    let network = Arc::new(Network::default());
    let as_user = |user| {
        let mut run = with(network.clone(), "scrobbler", 1);
        run.grants.user = Some(user);
        run.grants.rate_limits = vec![limit("1000/s", Per::User)];
        run
    };
    let reply = as_user(1).send(get(format!("http://127.0.0.1:{port}/busy"))).await.unwrap();
    assert_eq!(reply.status, 429);
    let refused = as_user(1).send(get(format!("http://127.0.0.1:{port}/"))).await.unwrap_err();
    assert_waits_two_minutes(refused);
    assert_eq!(body(&mut as_user(2), port, "/").await, "hello");
}

#[tokio::test]
async fn a_429_on_a_host_limit_pauses_every_plugin_it_binds() {
    let (port, hits) = server().await;
    let network = Arc::new(Network::default());
    let bound = |plugin| {
        let mut run = with(network.clone(), plugin, 1);
        run.grants.host_limits = vec![host_limit()];
        run
    };
    let mut told = bound("told");
    told.grants.rate_limits = vec![limit("1000/s", Per::Host)];
    told.grants.permissions.push(Permission::HostRateLimits);
    let reply = told.send(get(format!("http://127.0.0.1:{port}/busy"))).await;
    assert_eq!(reply.unwrap().status, 429);
    // One that declared nothing there waits in the host's queue too.
    let refused = bound("other").send(get(format!("http://127.0.0.1:{port}/"))).await;
    assert_waits_two_minutes(refused.unwrap_err());
    assert!(!hits.lock().unwrap().contains_key("/"));
    // One exempt from it has no host limit in its grants, and is not held up.
    assert_eq!(body(&mut with(network.clone(), "exempt", 1), port, "/").await, "hello");
}

#[tokio::test]
async fn a_429_counted_as_a_plugins_own_holds_up_no_plugin_the_host_limit_binds() {
    let (port, _) = server().await;
    let network = Arc::new(Network::default());
    let mut told = with(network.clone(), "told", 1);
    told.grants.host_limits = vec![host_limit()];
    let reply = told.send(get(format!("http://127.0.0.1:{port}/busy"))).await;
    assert_eq!(reply.unwrap().status, 429);
    assert_waits_two_minutes(
        told.send(get(format!("http://127.0.0.1:{port}/"))).await.unwrap_err(),
    );
    let mut other = with(network, "other", 1);
    other.grants.host_limits = vec![host_limit()];
    assert_eq!(body(&mut other, port, "/").await, "hello");
}

#[test]
fn a_request_waits_in_its_own_scope_and_in_any_host_limit_that_binds_it() {
    let host = Duration::from_secs(2);
    let mut run = run();
    run.grants.rate_limits = vec![limit("1/s", Per::Host)];
    let plugin = || Scope::Plugin("test".into());
    // Not allowed to limit every plugin, its own limit counts its own requests.
    assert_eq!(run.queues("127.0.0.1"), [(plugin(), Duration::from_secs(1))]);
    run.grants.permissions.push(Permission::HostRateLimits);
    // Allowed, but exempt, or not enabled where its limit would be in force: the same.
    assert_eq!(run.queues("127.0.0.1"), [(plugin(), Duration::from_secs(1))]);
    // In force, at the strictest any plugin set, it waits only in the host's queue.
    run.grants.host_limits = vec![("127.0.0.1".into(), host)];
    assert_eq!(run.queues("127.0.0.1"), [(Scope::Host, host)]);
    // Looser than its own, as in a check before its own is in force, it keeps to its own.
    run.grants.host_limits = vec![("127.0.0.1".into(), Duration::from_millis(500))];
    assert_eq!(run.queues("127.0.0.1"), [(Scope::Host, Duration::from_secs(1))]);
    run.grants.host_limits = vec![("127.0.0.1".into(), host)];

    // A user's limit, in a run for a user and in one for nobody, with the host's beside it.
    run.grants.rate_limits = vec![limit("1/s", Per::User)];
    assert_eq!(run.queues("127.0.0.1"), [(plugin(), Duration::from_secs(1)), (Scope::Host, host)]);
    run.grants.user = Some(7);
    let user = Scope::User("test".into(), 7);
    assert_eq!(run.queues("127.0.0.1"), [(user, Duration::from_secs(1)), (Scope::Host, host)]);

    // Nothing declared, nothing kept.
    assert_eq!(run.queues("localhost"), [(plugin(), Duration::ZERO)]);
}

#[tokio::test]
async fn a_429_after_a_redirect_pauses_where_it_led() {
    let (port, _) = server().await;
    let mut run = run();
    run.grants.destinations.push("localhost".into());
    let reply = run.send(get(format!("http://127.0.0.1:{port}/sent-on"))).await.unwrap();
    assert_eq!(reply.status, 429);
    let refused = run.send(get(format!("http://localhost:{port}/"))).await.unwrap_err();
    assert_waits_two_minutes(refused);
    // Where the request started would send the next one on into the same throttle, so it waits
    // too.
    let refused = run.send(get(format!("http://127.0.0.1:{port}/"))).await.unwrap_err();
    assert_waits_two_minutes(refused);
}

#[tokio::test]
async fn requests_keep_to_the_declared_pace() {
    let (port, _) = server().await;
    let mut run = run();
    let gap = Duration::from_millis(200);
    run.grants.rate_limits = vec![limit("5/s", Per::Plugin)];
    let started = Instant::now();
    for _ in 0..3 {
        assert_eq!(body(&mut run, port, "/").await, "hello");
    }
    assert!(started.elapsed() >= 2 * gap, "{:?}", started.elapsed());
}
