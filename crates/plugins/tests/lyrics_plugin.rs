//! The lyrics plugin (`plugins/lrclib-lyrics`), run by the real host with a library in memory.
//! No test here reaches the network: each one grants too little for a request to leave.
//!
//! The plugin is built against the 0.2 contract, so these also show that a 0.2 plugin still
//! runs. Needs the packed plugin, so these are ignored until it is built:
//!
//!     plugins/lrclib-lyrics/build.sh && cargo test -p jewelcase-plugins -- --ignored

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use jewelcase_plugins::{Album, Api, Artist, Event, Grants, Host, Library, Permission, Track};

const PLUGIN: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/lrclib-lyrics/target/lrclib-lyrics.wasm");

/// A library with one root, holding these tracks' files, and the plugin's state in memory.
struct Tracks {
    root: PathBuf,
    tracks: Vec<Track>,
    state: HashMap<String, Vec<u8>>,
}

impl Library for Tracks {
    async fn tracks(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Track>, String> {
        let after = self.tracks.iter().filter(|t| after.is_none_or(|after| t.id > after));
        Ok(after.take(limit as usize).cloned().collect())
    }

    async fn get_tracks(&mut self, ids: Vec<u64>) -> Result<Vec<Track>, String> {
        Ok(ids.iter().filter_map(|id| self.tracks.iter().find(|t| t.id == *id).cloned()).collect())
    }

    async fn albums(&mut self, _: Option<u64>, _: u32) -> Result<Vec<Album>, String> {
        Ok(Vec::new())
    }

    async fn artists(&mut self, _: Option<u64>, _: u32) -> Result<Vec<Artist>, String> {
        Ok(Vec::new())
    }

    async fn get_albums(&mut self, _: Vec<u64>) -> Result<Vec<Album>, String> {
        Ok(Vec::new())
    }

    async fn get_artists(&mut self, _: Vec<u64>) -> Result<Vec<Artist>, String> {
        Ok(Vec::new())
    }

    async fn album_tracks(&mut self, album: u64) -> Result<Vec<Track>, String> {
        Ok(self.tracks.iter().filter(|t| t.album_id == album).cloned().collect())
    }

    async fn artist_albums(&mut self, _: u64) -> Result<Vec<Album>, String> {
        Ok(Vec::new())
    }

    async fn roots(&mut self) -> Result<Vec<(u64, PathBuf)>, String> {
        Ok(vec![(7, self.root.clone())])
    }

    async fn state_get(&mut self, key: String) -> Result<Option<Vec<u8>>, String> {
        Ok(self.state.get(&key).cloned())
    }

    async fn state_set(&mut self, key: String, value: Vec<u8>) -> Result<(), String> {
        self.state.insert(key, value);
        Ok(())
    }

    async fn state_delete(&mut self, key: String) -> Result<(), String> {
        self.state.remove(&key);
        Ok(())
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
            album_id: 3,
            disc_number: 1,
            track_number: Some(id as u32),
            release_date: Some("2023".into()),
            isrc: None,
            has_lyrics: false,
            root: 7,
            path,
        }
    };
    Tracks {
        root: dir.to_path_buf(),
        tracks: vec![track(1, "Signal Part 1"), track(2, "Signal Part 2")],
        state: HashMap::new(),
    }
}

fn grants(permissions: &[Permission], destinations: &[&str]) -> Grants {
    Grants {
        plugin: "lrclib-lyrics".into(),
        library: 1,
        permissions: permissions.to_vec(),
        destinations: destinations.iter().map(|d| d.to_string()).collect(),
        rate_limits: Vec::new(),
        settings: Default::default(),
    }
}

#[tokio::test]
#[ignore = "needs plugins/lrclib-lyrics/build.sh"]
async fn a_missing_required_permission_stops_it_before_it_starts() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = Host::new()
        .unwrap()
        .run(
            Path::new(PLUGIN),
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
#[ignore = "needs plugins/lrclib-lyrics/build.sh"]
async fn the_network_reaches_only_approved_destinations() {
    let dir = tempfile::tempdir().unwrap();
    let permissions = [Permission::LibraryRead, Permission::Network, Permission::LibraryAdd];
    // The plugin asks lrclib.net, which this run does not approve, so nothing leaves.
    let outcome = Host::new()
        .unwrap()
        .run(
            Path::new(PLUGIN),
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
#[ignore = "needs plugins/lrclib-lyrics/build.sh"]
fn a_real_plugin_passes_the_install_check() {
    let host = Host::new().unwrap();
    let bytes = std::fs::read(PLUGIN).unwrap();
    host.check(&bytes, Api::V0_2).unwrap();
    // It is built against 0.2, whose `event` has no `playing`, so it does not fit 0.3.
    assert!(host.check(&bytes, Api::V0_3).is_err());
    assert!(host.check(b"not a component", Api::CURRENT).is_err());
}
