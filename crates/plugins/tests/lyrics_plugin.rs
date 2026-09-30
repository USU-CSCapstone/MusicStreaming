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

/// A library with one root, holding these tracks' files.
struct Tracks {
    root: PathBuf,
    tracks: Vec<Track>,
}

impl Library for Tracks {
    async fn tracks(&mut self, offset: u32, limit: u32) -> Result<Vec<Track>, String> {
        Ok(self.tracks.iter().skip(offset as usize).take(limit as usize).cloned().collect())
    }

    async fn roots(&mut self) -> Result<Vec<(u64, PathBuf)>, String> {
        Ok(vec![(7, self.root.clone())])
    }
}

fn library(dir: &Path) -> Tracks {
    let track = |id: u64, title: &str| {
        let path = format!("{title}.flac");
        std::fs::write(dir.join(&path), b"audio").unwrap();
        Track {
            id,
            title: title.into(),
            artists: vec!["Aurora Lane".into()],
            album: Some("Signal".into()),
            duration_ms: 200_000,
            has_lyrics: false,
            root: 7,
            path,
        }
    };
    Tracks {
        root: dir.to_path_buf(),
        tracks: vec![track(1, "Signal Part 1"), track(2, "Signal Part 2")],
    }
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
    assert!(outcome.touched.is_empty());
}

#[tokio::test]
#[ignore = "needs plugins/lrclib-lyrics/build.sh"]
async fn the_network_reaches_only_approved_destinations() {
    let dir = tempfile::tempdir().unwrap();
    let permissions = [Permission::LibraryRead, Permission::Network, Permission::LibraryAdd];
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
    assert!(outcome.touched.is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2, "only the audio");
}

#[test]
#[ignore = "needs plugins/lrclib-lyrics/build.sh"]
fn a_real_plugin_passes_the_install_check() {
    let host = Host::new().unwrap();
    host.check(&std::fs::read(PLUGIN).unwrap()).unwrap();
    assert!(host.check(b"not a component").is_err());
}
