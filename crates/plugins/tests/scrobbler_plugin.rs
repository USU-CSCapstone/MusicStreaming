//! The ListenBrainz plugin (`plugins/listenbrainz`), run by the real host. No test here reaches
//! the network: none grants it, so a request is refused before it leaves.
//!
//! Needs the packed plugin: run `plugins/listenbrainz/build.sh` first.

use std::path::{Path, PathBuf};
use std::time::Duration;

use jewelcase_plugins::{
    Album, Artist, Event, Grants, Host, Library, Outcome, Per, Permission, Play, PlayEnd, Playing,
    RunError, Track, manifest,
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
fn a_real_plugin_passes_the_install_check() {
    let (host, bytes) = (Host::<Empty>::new(None).unwrap(), std::fs::read(plugin()).unwrap());
    host.check(&bytes).unwrap();
}

/// The packed manifest installs, and paces each connected user's requests apart at the one call
/// a second ListenBrainz asks of each client application, halves of a refused batch included.
#[test]
fn it_asks_listenbrainz_at_most_once_a_second_for_each_user() {
    let manifest = manifest::read(&std::fs::read(plugin()).unwrap()).unwrap();
    let limits = manifest.rate_limits();
    let limits: Vec<_> = limits.iter().map(|l| (l.host.as_str(), l.gap, l.per)).collect();
    assert_eq!(limits, [("api.listenbrainz.org", Duration::from_secs(1), Per::User)]);
    assert_eq!(manifest.version, "0.1.0");
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
    Host::new(None).unwrap().run(plugin(), grants(permissions), Empty, event).await
}

#[tokio::test]
async fn a_start_reaches_the_plugin_as_playing() {
    let event = Event::Playing(Playing { track: track(&["Aurora Lane"]), started_at_ms: 1 });
    let outcome = run(&[Permission::LibraryRead, Permission::Playing], event).await;
    // It got as far as sending it, which the network it was not granted refused.
    let refused = RunError::Reported("Network access was not granted".into());
    assert_eq!(outcome.result, Err(refused));
}

#[tokio::test]
async fn a_start_with_no_artist_is_not_sent() {
    let event = Event::Playing(Playing { track: track(&[" "]), started_at_ms: 1 });
    let outcome = run(&[Permission::LibraryRead, Permission::Playing], event).await;
    let shown = "Nothing shown: the track has no usable title or artist.";
    assert_eq!(outcome.result, Ok(shown.to_owned()));
}

#[tokio::test]
async fn plays_with_no_artist_are_counted_and_not_sent() {
    let play = |artists: &[&str], listen_time_ms| Play {
        track: track(artists),
        started_at_ms: 1_790_942_400_000,
        listen_time_ms,
        end: PlayEnd::Finished,
    };
    let plays = vec![play(&[], 200_000), play(&[""], 200_000), play(&["Aurora Lane"], 1_000)];
    let outcome = run(&[Permission::LibraryRead, Permission::Played], Event::Played(plays)).await;
    // Nothing was left to send, so the network it was not granted was never asked.
    let sent = "Sent 0 listens; 1 plays were too short to count, 2 had no usable title or artist.";
    assert_eq!(outcome.result, Ok(sent.to_owned()));
}
