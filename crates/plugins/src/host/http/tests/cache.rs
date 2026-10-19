use std::sync::Arc;

use super::super::http::{self, Error, Host as _};
use super::{body, get, run, server, with};
use crate::manifest::Permission;
use crate::network::Network;

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
