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

/// A library of one track without lyrics, track 1.
fn library() -> Memory {
    Memory { tracks: vec![testing::track(1)], ..Memory::default() }
}

/// Runs the plugin on `host` over `library`.
async fn run_in(host: &Host<Memory>, library: &Memory) -> Outcome {
    assert!(Path::new(PLUGIN).is_file(), "run plugins/lrclib-lyrics/build.sh first");
    host.run(Path::new(PLUGIN), grants(), library.clone(), Event::Run).await
}

/// Runs the plugin on `host` over one track without lyrics, track 1.
async fn run(host: &Host<Memory>) -> Outcome {
    run_in(host, &library()).await
}

/// Keeps `status` and `body` as the reply to a request for `url`, as the host would have it.
fn answer(host: &Host<Memory>, url: &str, status: u16, body: &str) {
    let visible = Visible {
        library: 1,
        user: None,
        library_read: true,
        listening_activity: false,
        search_activity: false,
    };
    let reply = Reply {
        status,
        headers: vec![("cache-control".into(), "max-age=600".into())],
        body: body.as_bytes().to_vec().into(),
        url: url.into(),
        addr: None,
    };
    let key = Key::new(visible, url, &[]).unwrap();
    host.network.cache.store(key, reply, Instant::now(), SystemTime::now());
}

#[tokio::test]
async fn a_lookup_refused_as_rate_limited_ends_the_run() {
    let host = Host::new().unwrap();
    let plugin = Scope::Plugin("lrclib-lyrics".into());
    let (now, clock) = (Instant::now(), SystemTime::now());
    host.network.limits.answered("lrclib.net", &plugin, 429, Some("3600"), now, clock);
    let outcome = run(&host).await;
    let Err(RunError::Reported(why)) = &outcome.result else { panic!("{}", outcome.summary()) };
    // The pause less however long the run took to reach the request.
    let paused = "the service cannot be asked again for ";
    let seconds = why.strip_prefix(paused).and_then(|rest| rest.strip_suffix(" s"));
    let seconds: u64 = seconds.and_then(|s| s.parse().ok()).expect(why);
    assert!((1..=3600).contains(&seconds), "{why}");
}

/// A reply kept from a redirect to a host the plugin may not reach fails its lookup as `failed`,
/// which ends the run.
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
    let Err(RunError::Reported(why)) = &outcome.result else { panic!("{}", outcome.summary()) };
    assert!(why.contains("redirected elsewhere"), "{why}");
}

/// LRCLIB having nothing is a miss the plugin keeps with the time of the run, and the next run
/// does not look the track up again.
#[tokio::test]
async fn a_track_lrclib_has_nothing_for_is_not_looked_up_again() {
    let host = Host::new().unwrap();
    let base = "https://lrclib.net/api";
    answer(
        &host,
        &format!(
            "{base}/get?track_name=Track%201&artist_name=Aurora%20Lane&duration=200&album_name=Signal"
        ),
        404,
        "",
    );
    answer(
        &host,
        &format!("{base}/search?track_name=Track%201&artist_name=Aurora%20Lane"),
        200,
        "[]",
    );
    let library = library();
    let started = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();

    let first = run_in(&host, &library).await;
    assert_eq!(first.log.len(), 1, "{:?} {}", first.log, first.summary());
    assert!(first.log[0].contains("not found"), "{:?}", first.log);
    let kept = library.state.lock().unwrap().get("miss/1").cloned().expect("a miss is kept");
    let at = u64::from_le_bytes(kept.try_into().expect("8 bytes"));
    let finished = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
    assert!((started..=finished).contains(&at), "missed at {at}, in {started}..={finished}");

    let second = run_in(&host, &library).await;
    assert!(second.log.is_empty(), "{:?}", second.log);
}
