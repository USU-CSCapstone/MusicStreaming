//! Each plugin's HTTP client: kept for its later runs, in any library, while its destinations
//! stay the same, made again when they change, and never kept past an uninstall.

use std::path::Path;

use super::super::super::Host;
use super::super::super::testing::Memory;
use super::super::USER_AGENT;
use super::super::http::{Error, Host as _};
use super::{body, get, run, server};

/// What the test server answers to `/agent` when sent `user_agent`.
fn agent(user_agent: &str) -> String {
    user_agent.to_ascii_lowercase()
}

#[tokio::test]
async fn a_request_keeps_its_client_for_the_plugin() {
    let (port, _) = server().await;
    let mut run = run();
    assert_eq!(body(&mut run, port, "/agent").await, agent(USER_AGENT));
    let kept = run.client.lock().unwrap();
    let (kept_to, _) = kept.as_ref().expect("a client is kept");
    assert_eq!(kept_to, &run.grants.destinations);
}

/// A client told apart by its `User-Agent`, kept for the plugin with its destinations, is the
/// one a later run of it, in another library, sends with.
#[tokio::test]
async fn a_later_run_uses_the_client_kept_for_its_plugin() {
    let (port, _) = server().await;
    let mut run = run();
    run.grants.library = 2;
    let kept = reqwest::Client::builder().user_agent("Kept").build().unwrap();
    *run.client.lock().unwrap() = Some((run.grants.destinations.clone(), kept));
    assert_eq!(body(&mut run, port, "/agent").await, "kept");
}

/// A client kept to more destinations than a run's is not the one it sends with, since that
/// would follow a redirect the run may not.
#[tokio::test]
async fn changed_destinations_get_a_client_that_keeps_to_them() {
    let (port, _) = server().await;
    let mut wider = run();
    wider.grants.destinations.push("localhost".into());
    assert_eq!(body(&mut wider, port, "/away").await, "hello");
    let mut narrower = run();
    narrower.client = wider.client.clone();
    let failed = narrower.send(get(format!("http://127.0.0.1:{port}/away"))).await.unwrap_err();
    assert!(
        matches!(&failed, Error::Failed(why) if why.contains("redirected elsewhere: localhost is not one of")),
        "{failed:?}"
    );
    let kept = narrower.client.lock().unwrap();
    assert_eq!(kept.as_ref().map(|(kept_to, _)| kept_to), Some(&narrower.grants.destinations));
}

/// A run still under way when its plugin is uninstalled keeps its client with the entry it
/// started with, which is gone, so the plugin's next run starts without one.
#[tokio::test]
async fn a_run_under_way_at_uninstall_leaves_no_client_behind() {
    let (port, _) = server().await;
    let file = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/tests/sandbox.wat"));
    let host = Host::<Memory>::new().unwrap();
    let mut under_way = run();
    let (_, client) = host.compiled("test", file).await.unwrap();
    under_way.client = client;
    host.uninstalled("test");
    assert_eq!(body(&mut under_way, port, "/agent").await, agent(USER_AGENT));
    let (_, client) = host.compiled("test", file).await.unwrap();
    assert!(client.lock().unwrap().is_none());
}
