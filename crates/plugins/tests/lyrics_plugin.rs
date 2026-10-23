//! The lyrics plugin (`plugins/lrclib-lyrics`), run by the real host with a library in memory.
//! No test here reaches the network: each one grants too little for a request to leave.
//!
//! Needs the packed plugin: run `plugins/lrclib-lyrics/build.sh` first.

mod common;

use std::path::Path;
use std::time::Duration;

use common::{grants, plugin, track};
use jewelcase_plugins::testing::Memory;
use jewelcase_plugins::{Changes, Event, Host, Per, Permission, RunError, manifest};

/// Two tracks without lyrics, with their audio files in `dir`.
fn library(dir: &Path) -> Memory {
    let tracks = vec![track(1, "Signal Part 1"), track(2, "Signal Part 2")];
    for t in &tracks {
        std::fs::write(dir.join(&t.path), b"audio").unwrap();
    }
    Memory { root: dir.to_path_buf(), tracks, ..Memory::default() }
}

/// A lookup that can't be answered now, as without the network, ends the run with an error at
/// the first track, before it counts as looked up: the pass keeps its place before it, and a
/// change is delivered again. Nothing is recorded as a miss.
#[tokio::test]
async fn a_lookup_that_cannot_be_answered_now_ends_the_run_with_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let not_granted = || RunError::Reported("a permission it needs was not granted".into());
    let changes = || Changes { changed: vec![1, 2], removed: vec![] };
    for event in [Event::Run, Event::Scheduled, Event::TracksChanged(changes())] {
        let library = library(dir.path());
        let grants = grants(&[Permission::LibraryRead, Permission::LibraryAdd], &["lrclib.net"]);
        let outcome = Host::new(None).unwrap().run(plugin(), grants, library.clone(), event).await;
        assert_eq!(outcome.result, Err(not_granted()));
        assert!(outcome.log.is_empty(), "{:?}", outcome.log);
        assert!(outcome.touched.is_empty());
        assert!(library.state.lock().unwrap().is_empty(), "no place moved on, no miss");
    }
}

/// The packed manifest installs, and keeps to lrclib.net's published allowance for a client
/// that doesn't identify itself with a homepage link or email, as the host's requests don't:
/// about 45 requests a minute from one address.
#[test]
fn it_keeps_to_lrclibs_allowance_for_an_unidentified_client() {
    let manifest = manifest::read(&std::fs::read(plugin()).unwrap()).unwrap();
    let limits = manifest.rate_limits();
    let limits: Vec<_> = limits.iter().map(|l| (l.host.as_str(), l.gap, l.per)).collect();
    assert_eq!(limits, [("lrclib.net", Duration::from_secs(60) / 45, Per::Plugin)]);
}

#[tokio::test]
async fn the_network_reaches_only_approved_destinations() {
    let dir = tempfile::tempdir().unwrap();
    let permissions = [Permission::LibraryRead, Permission::Network, Permission::LibraryAdd];
    // The plugin asks lrclib.net, which this run does not approve, so nothing leaves.
    let outcome = Host::new(None)
        .unwrap()
        .run(plugin(), grants(&permissions, &["example.invalid"]), library(dir.path()), Event::Run)
        .await;
    assert!(outcome.result.is_ok(), "{}", outcome.summary());
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
    let host = Host::<Memory>::new(None).unwrap();
    let bytes = std::fs::read(plugin()).unwrap();
    host.check(&bytes).unwrap();
    assert!(host.check(b"not a component").is_err());
}
