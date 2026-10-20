//! What the host's tests share: a library held in memory, a plugin's grants, and a run over it
//! built without a plugin, so a test calls the host imports directly.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use wasmtime::StoreLimitsBuilder;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::WasiCtx;

use super::{Album, Artist, Grants, Library, Run, Track};
use crate::manifest::Permission;

/// A library held in memory: its catalog, one root (ID 1) at `root`, and the plugin's state.
#[derive(Default)]
pub struct Memory {
    pub root: PathBuf,
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub state: HashMap<String, Vec<u8>>,
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

/// The grants of plugin `test` in library 1: `permissions`, with 127.0.0.1 its only
/// destination, for no user, with no rate limits or settings.
pub fn grants(permissions: &[Permission]) -> Grants {
    Grants {
        plugin: "test".into(),
        library: 1,
        user: None,
        permissions: permissions.to_vec(),
        destinations: vec!["127.0.0.1".into()],
        rate_limits: Vec::new(),
        host_limits: Vec::new(),
        settings: Default::default(),
    }
}

/// A run over `library` with `grants(permissions)`, and a network of its own.
pub fn run<L: Library>(library: L, permissions: &[Permission]) -> Run<L> {
    Run {
        wasi: WasiCtx::builder().build(),
        table: ResourceTable::new(),
        limits: StoreLimitsBuilder::new().build(),
        ticks: 0,
        grants: grants(permissions),
        library,
        client: None,
        network: Arc::default(),
        roots: None,
        log: Vec::new(),
        written: 0,
        touched: Vec::new(),
    }
}
