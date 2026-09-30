//! The lyrics plugin (`plugins/lrclib-lyrics`), run by the real host with a library in memory.
//! No test here reaches the network: each one grants too little for a request to leave.
//!
//! Needs the packed plugin, so these are ignored until it is built:
//!
//!     plugins/lrclib-lyrics/build.sh && cargo test -p jewelcase-plugins -- --ignored

use std::path::{Path, PathBuf};

use jewelcase_plugins::{Grants, Host, Library, Permission, Track};

const PLUGIN: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/lrclib-lyrics/target/lrclib-lyrics.wasm");

struct Tracks(Vec<(Track, PathBuf)>);

impl Library for Tracks {
    async fn tracks(&mut self, offset: u32, limit: u32) -> Result<Vec<Track>, String> {
        let page = self.0.iter().skip(offset as usize).take(limit as usize);
        Ok(page.map(|(track, _)| track.clone()).collect())
    }

    async fn audio_without_lyrics(&mut self, track: u64) -> Result<PathBuf, String> {
        let found = self.0.iter().find(|(t, _)| t.id == track && !t.has_lyrics);
        found.map(|(_, audio)| audio.clone()).ok_or_else(|| "no such track".into())
    }
}

fn library(dir: &Path) -> Tracks {
    let track = |id: u64, title: &str| {
        let audio = dir.join(format!("{title}.flac"));
        std::fs::write(&audio, b"audio").unwrap();
        let track = Track {
            id,
            title: title.into(),
            artists: vec!["Aurora Lane".into()],
            album: Some("Signal".into()),
            duration_ms: 200_000,
            has_lyrics: false,
        };
        (track, audio)
    };
    Tracks(vec![track(1, "Signal Part 1"), track(2, "Signal Part 2")])
}

fn grants(permissions: &[Permission], destinations: &[&str]) -> Grants {
    Grants {
        permissions: permissions.to_vec(),
        destinations: destinations.iter().map(|d| d.to_string()).collect(),
    }
}

#[tokio::test]
#[ignore = "needs plugins/lrclib-lyrics/build.sh"]
async fn a_missing_required_permission_stops_it_before_it_starts() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = Host::new()
        .unwrap()
        .run(Path::new(PLUGIN), grants(&[Permission::LibraryRead], &[]), library(dir.path()))
        .await;
    assert!(!outcome.ok);
    assert!(outcome.summary.contains("needs permission to use the network"), "{}", outcome.summary);
    assert!(outcome.saved.is_empty());
}

#[tokio::test]
#[ignore = "needs plugins/lrclib-lyrics/build.sh"]
async fn the_network_reaches_only_approved_destinations() {
    let dir = tempfile::tempdir().unwrap();
    let permissions = [Permission::LibraryRead, Permission::Network, Permission::LibraryWrite];
    // The plugin asks lrclib.net, which this run does not approve, so nothing leaves.
    let outcome = Host::new()
        .unwrap()
        .run(Path::new(PLUGIN), grants(&permissions, &["example.invalid"]), library(dir.path()))
        .await;
    assert!(outcome.ok, "{}", outcome.summary);
    assert_eq!(outcome.log.len(), 2, "{:?}", outcome.log);
    for line in &outcome.log {
        assert!(
            line.contains("lrclib.net is not one of this plugin's approved destinations"),
            "{line}"
        );
    }
    assert!(outcome.saved.is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2, "only the audio");
}

#[test]
#[ignore = "needs plugins/lrclib-lyrics/build.sh"]
fn a_real_plugin_passes_the_install_check() {
    let host = Host::new().unwrap();
    host.check(&std::fs::read(PLUGIN).unwrap()).unwrap();
    assert!(host.check(b"not a component").is_err());
}
