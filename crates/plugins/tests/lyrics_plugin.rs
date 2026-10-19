//! The lyrics plugin (`plugins/lrclib-lyrics`), run by the real host with a library in memory.
//! No test here reaches the network: each one grants too little for a request to leave.
//!
//! The plugin is built against the 0.2 contract, so these also show that a 0.2 plugin still
//! runs. Needs the packed plugin: run `plugins/lrclib-lyrics/build.sh` first.

mod common;

use std::path::Path;

use common::{State, Tracks, grants, plugin, track};
use jewelcase_plugins::{Api, Event, Host, Permission};

/// Two tracks without lyrics, with their audio files in `dir`.
fn library(dir: &Path) -> Tracks {
    let tracks = vec![track(1, "Signal Part 1"), track(2, "Signal Part 2")];
    for t in &tracks {
        std::fs::write(dir.join(&t.path), b"audio").unwrap();
    }
    Tracks { root: dir.to_path_buf(), tracks, state: State::default(), fail_after: None }
}

#[tokio::test]
async fn a_missing_required_permission_stops_it_before_it_starts() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = Host::new()
        .unwrap()
        .run(
            plugin(),
            Api::V0_2,
            grants(&[Permission::LibraryRead], &[]),
            library(dir.path()),
            Event::Run,
        )
        .await;
    assert!(!outcome.ok);
    assert!(outcome.summary.contains("needs permission to use the network"), "{}", outcome.summary);
    assert!(outcome.touched.is_empty());
}

#[tokio::test]
async fn the_network_reaches_only_approved_destinations() {
    let dir = tempfile::tempdir().unwrap();
    let permissions = [Permission::LibraryRead, Permission::Network, Permission::LibraryAdd];
    // The plugin asks lrclib.net, which this run does not approve, so nothing leaves.
    let outcome = Host::new()
        .unwrap()
        .run(
            plugin(),
            Api::V0_2,
            grants(&permissions, &["example.invalid"]),
            library(dir.path()),
            Event::Run,
        )
        .await;
    assert!(outcome.ok, "{}", outcome.summary);
    assert_eq!(outcome.log.len(), 2, "{:?}", outcome.log);
    for line in &outcome.log {
        assert!(
            line.contains("lrclib.net is not one of this plugin's approved destinations"),
            "{line}"
        );
    }
    assert!(outcome.touched.is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2, "only the audio");
}

#[test]
fn a_real_plugin_passes_the_install_check() {
    let host = Host::new().unwrap();
    let bytes = std::fs::read(plugin()).unwrap();
    host.check(&bytes, Api::V0_2).unwrap();
    // It is built against 0.2, whose `http.send` answers a message, so it does not fit 0.3.
    assert!(host.check(&bytes, Api::V0_3).is_err());
    assert!(host.check(b"not a component", Api::CURRENT).is_err());
}
