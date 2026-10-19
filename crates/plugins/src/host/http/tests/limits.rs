use std::sync::Arc;
use std::time::{Duration, Instant};

use super::super::http::{Error, Host as _};
use super::{body, get, run, server, with};
use crate::manifest::{Per, Permission, RateLimit, rate_limit_gap};
use crate::network::Network;
use crate::network::limits::Scope;

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
