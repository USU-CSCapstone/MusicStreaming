//! What the lyrics plugin's tests share: the packed plugin, and a library in memory for it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jewelcase_plugins::{Album, Artist, Grants, Library, Permission, Track};

/// The packed plugin. Panics, saying how to build it, when it is not built.
pub fn plugin() -> &'static Path {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/lrclib-lyrics/target/lrclib-lyrics.wasm"
    ));
    assert!(path.is_file(), "run plugins/lrclib-lyrics/build.sh first");
    path
}

/// The plugin's state, in memory, kept from one run to the next.
pub type State = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// A library with one root, holding these tracks' files, and the plugin's state.
#[derive(Clone)]
pub struct Tracks {
    pub root: PathBuf,
    pub tracks: Vec<Track>,
    pub state: State,
    /// Reading the page after this track fails, as a database error would.
    pub fail_after: Option<u64>,
}

impl Library for Tracks {
    async fn tracks(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Track>, String> {
        if after.is_some() && after == self.fail_after {
            return Err("the library could not be read".into());
        }
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
        Ok(self.state.lock().unwrap().get(&key).cloned())
    }

    async fn state_set(&mut self, key: String, value: Vec<u8>) -> Result<(), String> {
        self.state.lock().unwrap().insert(key, value);
        Ok(())
    }

    async fn state_delete(&mut self, key: String) -> Result<(), String> {
        self.state.lock().unwrap().remove(&key);
        Ok(())
    }
}

/// Track `id`, titled `title`, with no lyrics and no file.
pub fn track(id: u64, title: &str) -> Track {
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
        path: format!("{title}.flac"),
    }
}

/// The lyrics plugin's grants of `permissions`, reaching `destinations`, in library 1.
pub fn grants(permissions: &[Permission], destinations: &[&str]) -> Grants {
    Grants {
        plugin: "lrclib-lyrics".into(),
        library: 1,
        user: None,
        permissions: permissions.to_vec(),
        destinations: destinations.iter().map(|d| d.to_string()).collect(),
        rate_limits: Vec::new(),
        host_limits: Vec::new(),
        settings: Default::default(),
    }
}
