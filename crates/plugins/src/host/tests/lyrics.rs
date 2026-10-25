//! The real lyrics plugin (`plugins/lrclib-lyrics`), granted the network to lrclib.net, with
//! what the host would answer its first request set up beforehand, so nothing is sent.
//!
//! Needs the packed plugin: run `plugins/lrclib-lyrics/build.sh` first.

use std::path::Path;
use std::time::{Instant, SystemTime};

use super::super::testing::{self, Memory};
use super::super::{Event, Grants, Host, Outcome, RunError};
use crate::manifest::Permission;
use crate::network::cache::{Key, Reply, Visible};
use crate::network::limits::Scope;

const PLUGIN: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/lrclib-lyrics/target/lrclib-lyrics.wasm");

/// The plugin's grants in library 1: reading it, and the network to lrclib.net.
fn grants() -> Grants {
    let permissions = [Permission::LibraryRead, Permission::Network];
    Grants::new("lrclib-lyrics", 1, &permissions, &["lrclib.net"])
}

/// Runs the plugin on `host` over one track without lyrics, track 1.
async fn run(host: &Host<Memory>) -> Outcome {
    assert!(Path::new(PLUGIN).is_file(), "run plugins/lrclib-lyrics/build.sh first");
    let library = Memory { tracks: vec![testing::track(1)], ..Memory::default() };
    host.run(Path::new(PLUGIN), grants(), library, Event::Run).await
}

#[tokio::test]
async fn a_lookup_refused_as_rate_limited_ends_the_run_as_an_outage() {
    let host = Host::new().unwrap();
    let plugin = Scope::Plugin("lrclib-lyrics".into());
    let (now, clock) = (Instant::now(), SystemTime::now());
    host.network.limits.answered("lrclib.net", &plugin, 429, Some("3600"), now, clock);
    let outcome = run(&host).await;
    assert!(outcome.outage, "{}", outcome.summary());
    let Err(RunError::Reported(why)) = &outcome.result else { panic!("{}", outcome.summary()) };
    // The pause less however long the run took to reach the request.
    let paused = "the service cannot be asked again for ";
    let seconds = why.strip_prefix(paused).and_then(|rest| rest.strip_suffix(" s"));
    let seconds: u64 = seconds.and_then(|s| s.parse().ok()).expect(why);
    assert!((1..=3600).contains(&seconds), "{why}");
}

/// A reply kept from a redirect to a host the plugin may not reach fails its lookup as `failed`,
/// which ends the run, though it is no outage.
#[tokio::test]
async fn a_lookup_that_failed_ends_the_run() {
    let host = Host::new().unwrap();
    let visible = Visible {
        library: 1,
        user: None,
        library_read: true,
        listening_activity: false,
        search_activity: false,
    };
    let url = "https://lrclib.net/api/get?track_name=Track%201&artist_name=Aurora%20Lane\
               &duration=200&album_name=Signal";
    let key = Key::new(visible, url, &[]).unwrap();
    let reply = Reply {
        status: 200,
        headers: vec![("cache-control".into(), "max-age=60".into())],
        body: Default::default(),
        url: "https://elsewhere.example/".into(),
        addr: None,
    };
    host.network.cache.store(key, reply, Instant::now(), SystemTime::now());
    let outcome = run(&host).await;
    assert!(!outcome.outage, "{}", outcome.summary());
    let Err(RunError::Reported(why)) = &outcome.result else { panic!("{}", outcome.summary()) };
    assert!(why.contains("redirected elsewhere"), "{why}");
}
