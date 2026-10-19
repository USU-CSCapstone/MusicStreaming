//! The ListenBrainz plugin (`plugins/listenbrainz`), built against the 0.3 contract, run by the
//! real host. No test here reaches the network: none grants it, so a request is refused before
//! it leaves.
//!
//! Needs the packed plugin: run `plugins/listenbrainz/build.sh` first.

use std::path::{Path, PathBuf};

use jewelcase_plugins::{
    Album, Api, Artist, Event, Grants, Host, Library, Outcome, Permission, Play, PlayEnd, Playing,
    Track,
};

/// The packed plugin. Panics, saying how to build it, when it is not built.
fn plugin() -> &'static Path {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/listenbrainz/target/listenbrainz.wasm"
    ));
    assert!(path.is_file(), "run plugins/listenbrainz/build.sh first");
    path
}

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
fn a_0_3_plugin_passes_only_the_0_3_install_check() {
    let (host, bytes) = (Host::new().unwrap(), std::fs::read(plugin()).unwrap());
    host.check(&bytes, Api::V0_3).unwrap();
    assert!(host.check(&bytes, Api::V0_2).is_err(), "0.2 has no playing event to hand it");
}

/// User 1's grants of `permissions` to the plugin in library 1, with their token set.
fn grants(permissions: &[Permission]) -> Grants {
    let mut grants = Grants {
        plugin: "listenbrainz".into(),
        library: 1,
        user: Some(1),
        permissions: permissions.to_vec(),
        destinations: vec!["api.listenbrainz.org".into()],
        rate_limits: Vec::new(),
        host_limits: Vec::new(),
        settings: Default::default(),
    };
    grants.settings.insert("token".into(), "a-token".into());
    grants
}

/// A track by `artists`, 200 s long.
fn track(artists: &[&str]) -> Track {
    Track {
        id: 1,
        title: "Signal".into(),
        artists: artists.iter().map(|a| a.to_string()).collect(),
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
    }
}

/// Runs the plugin for user 1, granted `permissions`, on `event`.
async fn run(permissions: &[Permission], event: Event) -> Outcome {
    Host::new().unwrap().run(plugin(), Api::V0_3, grants(permissions), Empty, event).await
}

#[tokio::test]
async fn a_start_reaches_the_plugin_as_playing() {
    let event = Event::Playing(Playing { track: track(&["Aurora Lane"]), started_at: 1 });
    let outcome = run(&[Permission::LibraryRead, Permission::Playing], event).await;
    // It got as far as sending it, which the network it was not granted refused.
    assert!(!outcome.ok);
    assert_eq!(outcome.summary, "the plugin reported an error: Network access was not granted");
}

#[tokio::test]
async fn a_start_with_no_artist_is_not_sent() {
    let event = Event::Playing(Playing { track: track(&[" "]), started_at: 1 });
    let outcome = run(&[Permission::LibraryRead, Permission::Playing], event).await;
    assert!(outcome.ok, "{}", outcome.summary);
    assert_eq!(outcome.summary, "Nothing shown: the track has no usable title or artist.");
}

#[tokio::test]
async fn plays_with_no_artist_are_counted_and_not_sent() {
    let play = |artists: &[&str], listen_time_ms| Play {
        track: track(artists),
        started_at: 1_790_942_400_000,
        listen_time_ms,
        end: PlayEnd::Finished,
    };
    let plays = vec![play(&[], 200_000), play(&[""], 200_000), play(&["Aurora Lane"], 1_000)];
    let outcome = run(&[Permission::LibraryRead, Permission::Played], Event::Played(plays)).await;
    // Nothing was left to send, so the network it was not granted was never asked.
    assert!(outcome.ok, "{}", outcome.summary);
    assert_eq!(
        outcome.summary,
        "Sent 0 listens; 1 plays were too short to count, 2 had no usable title or artist."
    );
}
