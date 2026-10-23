//! What this crate's tests share: a library held in memory, a catalog to fill it with, and
//! grants built in one call.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{Album, Artist, Grants, Library, Permission, Track};

/// A plugin's state, kept in memory from one run to the next.
pub type State = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// A library held in memory: its catalog, one root (ID 1) at `root`, and the plugin's state,
/// which every clone shares, so a test reads what a run left behind.
#[derive(Clone, Default)]
pub struct Memory {
    pub root: PathBuf,
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub state: State,
    /// Reading the page of tracks after this one fails, as a database error would.
    pub fail_after: Option<u64>,
}

/// The first `limit` of `items` with an ID after `after`, as a page is read. `items` must be in
/// ID order, as a library's catalog is.
fn page<T: Clone>(items: &[T], id: fn(&T) -> u64, after: Option<u64>, limit: u32) -> Vec<T> {
    let after = items.iter().filter(|item| after.is_none_or(|after| id(item) > after));
    after.take(limit as usize).cloned().collect()
}

/// Those of `items` with these IDs, in the order asked for.
fn by_id<T: Clone>(items: &[T], id: fn(&T) -> u64, ids: &[u64]) -> Vec<T> {
    ids.iter().filter_map(|want| items.iter().find(|item| id(item) == *want).cloned()).collect()
}

impl Library for Memory {
    async fn tracks(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Track>, String> {
        if after.is_some() && after == self.fail_after {
            return Err("the library could not be read".into());
        }
        Ok(page(&self.tracks, |t| t.id, after, limit))
    }

    async fn get_tracks(&mut self, ids: Vec<u64>) -> Result<Vec<Track>, String> {
        Ok(by_id(&self.tracks, |t| t.id, &ids))
    }

    async fn albums(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Album>, String> {
        Ok(page(&self.albums, |a| a.id, after, limit))
    }

    async fn artists(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Artist>, String> {
        Ok(page(&self.artists, |a| a.id, after, limit))
    }

    async fn get_albums(&mut self, ids: Vec<u64>) -> Result<Vec<Album>, String> {
        Ok(by_id(&self.albums, |a| a.id, &ids))
    }

    async fn get_artists(&mut self, ids: Vec<u64>) -> Result<Vec<Artist>, String> {
        Ok(by_id(&self.artists, |a| a.id, &ids))
    }

    async fn album_tracks(&mut self, album: u64) -> Result<Vec<Track>, String> {
        Ok(self.tracks.iter().filter(|t| t.album_id == album).cloned().collect())
    }

    /// The albums that credit the artist by name.
    async fn artist_albums(&mut self, artist: u64) -> Result<Vec<Album>, String> {
        let name = self.artists.iter().find(|a| a.id == artist).and_then(|a| a.name.clone());
        let credits = |album: &&Album| name.as_ref().is_some_and(|n| album.artists.contains(n));
        Ok(self.albums.iter().filter(credits).cloned().collect())
    }

    async fn roots(&mut self) -> Result<Vec<(u64, PathBuf)>, String> {
        Ok(vec![(1, self.root.clone())])
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

/// Track `id`, on album 1, at `{id}.flac` in root 1.
pub fn track(id: u64) -> Track {
    Track {
        id,
        title: format!("Track {id}"),
        artists: vec!["Aurora Lane".into()],
        album_id: 1,
        album: Some("Signal".into()),
        disc_number: 1,
        track_number: None,
        release_date: None,
        isrc: None,
        duration_ms: 200_000,
        has_lyrics: false,
        root: 1,
        path: format!("{id}.flac"),
    }
}

/// Album `id`, by Aurora Lane.
pub fn album(id: u64) -> Album {
    Album {
        id,
        title: Some(format!("Album {id}")),
        artists: vec!["Aurora Lane".into()],
        release_date: None,
        has_artwork: false,
    }
}

/// Artist `id`, named `Artist {id}`.
pub fn artist(id: u64) -> Artist {
    Artist { id, name: Some(format!("Artist {id}")), has_image: false, has_biography: false }
}

impl Grants {
    /// The grants of `plugin` in `library`: `permissions`, reaching `destinations`, for no user,
    /// with no rate limits or settings.
    pub fn new(
        plugin: &str,
        library: i64,
        permissions: &[Permission],
        destinations: &[&str],
    ) -> Self {
        Grants {
            plugin: plugin.into(),
            library,
            user: None,
            permissions: permissions.to_vec(),
            destinations: destinations.iter().map(|d| d.to_string()).collect(),
            rate_limits: Vec::new(),
            host_limits: Vec::new(),
            settings: Default::default(),
        }
    }
}
