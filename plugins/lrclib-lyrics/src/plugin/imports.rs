//! The pass's [`Store`] in a run: the plugin's `library` and `state` imports.

use super::jewelcase::plugin::library::{self, Track};
use super::jewelcase::plugin::state;
use crate::pass::Store;

/// The run's own library and state.
pub struct Imports;

impl Store for Imports {
    type Track = Track;

    fn id(track: &Track) -> u64 {
        track.id
    }

    fn has_lyrics(track: &Track) -> bool {
        track.has_lyrics
    }

    fn tracks(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Track>, String> {
        library::tracks(after, limit).map_err(String::from)
    }

    fn get_tracks(&mut self, ids: &[u64]) -> Result<Vec<Track>, String> {
        library::get_tracks(ids).map_err(String::from)
    }

    fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
        state::get(key).map_err(String::from)
    }

    fn set(&mut self, key: &str, value: &[u8]) -> Result<(), String> {
        state::set(key, value).map_err(String::from)
    }

    fn delete(&mut self, key: &str) -> Result<(), String> {
        state::delete(key).map_err(String::from)
    }
}
