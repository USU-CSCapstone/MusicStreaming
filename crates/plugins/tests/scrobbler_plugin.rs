//! The ListenBrainz plugin (`plugins/listenbrainz`), built against the 0.3 contract, run by the
//! real host. No test here reaches the network: none grants it, so a request is refused before
//! it leaves.
//!
//! Needs the packed plugin, so these are ignored until it is built:
//!
//!     plugins/listenbrainz/build.sh && cargo test -p jewelcase-plugins -- --ignored

use std::path::{Path, PathBuf};

use jewelcase_plugins::{
    Album, Api, Artist, Event, Grants, Host, Library, Permission, Playing, Track,
};

const PLUGIN: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/listenbrainz/target/listenbrainz.wasm");

/// A library with nothing in it: the event carries all the plugin needs.
struct Empty;

impl Library for Empty {
    async fn tracks(&mut self, _: Option<u64>, _: u32) -> Result<Vec<Track>, String> {
        Ok(Vec::new())
    }

    async fn get_tracks(&mut self, _: Vec<u64>) -> Result<Vec<Track>, String> {
        Ok(Vec::new())
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

    async fn album_tracks(&mut self, _: u64) -> Result<Vec<Track>, String> {
        Ok(Vec::new())
    }

    async fn artist_albums(&mut self, _: u64) -> Result<Vec<Album>, String> {
        Ok(Vec::new())
    }

    async fn roots(&mut self) -> Result<Vec<(u64, PathBuf)>, String> {
        Ok(Vec::new())
    }

    async fn state_get(&mut self, _: String) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }

    async fn state_set(&mut self, _: String, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }

    async fn state_delete(&mut self, _: String) -> Result<(), String> {
        Ok(())
    }
}

#[test]
#[ignore = "needs plugins/listenbrainz/build.sh"]
fn a_0_3_plugin_passes_only_the_0_3_install_check() {
    let (host, bytes) = (Host::new().unwrap(), std::fs::read(PLUGIN).unwrap());
    host.check(&bytes, Api::V0_3).unwrap();
    assert!(host.check(&bytes, Api::V0_2).is_err(), "0.2 has no playing event to hand it");
}

#[tokio::test]
#[ignore = "needs plugins/listenbrainz/build.sh"]
async fn a_start_reaches_the_plugin_as_playing() {
    let mut grants = Grants {
        plugin: "listenbrainz".into(),
        library: 1,
        permissions: vec![Permission::LibraryRead, Permission::Playing],
        destinations: vec!["api.listenbrainz.org".into()],
        rate_limits: Vec::new(),
        settings: Default::default(),
    };
    grants.settings.insert("token".into(), "a-token".into());
    let track = Track {
        id: 1,
        title: "Signal".into(),
        artists: vec!["Aurora Lane".into()],
        album: Some("Signal".into()),
        duration_ms: 200_000,
        album_id: 3,
        disc_number: 1,
        track_number: Some(1),
        release_date: None,
        isrc: None,
        has_lyrics: false,
        root: 7,
        path: "Signal.flac".into(),
    };
    let event = Event::Playing(Playing { track, started_at: 1_790_942_400_000 });
    let outcome =
        Host::new().unwrap().run(Path::new(PLUGIN), Api::V0_3, grants, Empty, event).await;
    // It got as far as sending it, which the network it was not granted refused.
    assert!(!outcome.ok);
    assert_eq!(outcome.summary, "the plugin reported an error: Network access was not granted");
}
