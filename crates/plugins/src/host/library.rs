//! The imports that reach the library: its catalog (`library`), its folders (`files`), and the
//! plugin's own state for it (`state`). Each checks its permission before anything else.

use super::bindings::jewelcase::plugin::{files, library, state};
use super::{Album, Artist, Library, Run, Track};
use crate::files as library_files;
use crate::manifest::Permission;

/// The most one page, or one `get-tracks`, `get-albums`, or `get-artists`, holds, whatever the
/// plugin asks for.
const PAGE_LIMIT: u32 = 500;
/// The longest state key, and the largest value.
const MAX_KEY: usize = 512;
const MAX_VALUE: usize = 1 << 20;

impl<L: Library> library::Host for Run<L> {
    async fn tracks(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Track>, String> {
        self.may(Permission::LibraryRead)?;
        self.library.tracks(after, limit.min(PAGE_LIMIT)).await
    }

    async fn get_tracks(&mut self, mut ids: Vec<u64>) -> Result<Vec<Track>, String> {
        self.may(Permission::LibraryRead)?;
        ids.truncate(PAGE_LIMIT as usize);
        self.library.get_tracks(ids).await
    }

    async fn albums(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Album>, String> {
        self.may(Permission::LibraryRead)?;
        self.library.albums(after, limit.min(PAGE_LIMIT)).await
    }

    async fn artists(&mut self, after: Option<u64>, limit: u32) -> Result<Vec<Artist>, String> {
        self.may(Permission::LibraryRead)?;
        self.library.artists(after, limit.min(PAGE_LIMIT)).await
    }

    async fn get_albums(&mut self, mut ids: Vec<u64>) -> Result<Vec<Album>, String> {
        self.may(Permission::LibraryRead)?;
        ids.truncate(PAGE_LIMIT as usize);
        self.library.get_albums(ids).await
    }

    async fn get_artists(&mut self, mut ids: Vec<u64>) -> Result<Vec<Artist>, String> {
        self.may(Permission::LibraryRead)?;
        ids.truncate(PAGE_LIMIT as usize);
        self.library.get_artists(ids).await
    }

    async fn album_tracks(&mut self, album: u64) -> Result<Vec<Track>, String> {
        self.may(Permission::LibraryRead)?;
        self.library.album_tracks(album).await
    }

    async fn artist_albums(&mut self, artist: u64) -> Result<Vec<Album>, String> {
        self.may(Permission::LibraryRead)?;
        self.library.artist_albums(artist).await
    }
}

/// Runs file work off the plugin runtime's thread, so one plugin's disk waits hold up no other.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work).await.map_err(|_| "the file operation failed".to_owned())?
}

impl<L: Library> files::Host for Run<L> {
    async fn roots(&mut self) -> Result<Vec<u64>, String> {
        let file_permissions =
            [Permission::LibraryRead, Permission::LibraryAdd, Permission::LibraryChange];
        if !file_permissions.iter().any(|p| self.grants.permissions.contains(p)) {
            return Err(format!("{} was not granted", Permission::LibraryRead.title()));
        }
        Ok(Run::roots(self).await?.iter().map(|(id, _)| *id).collect())
    }

    async fn list(&mut self, root: u64, folder: String) -> Result<Vec<files::Entry>, String> {
        self.may(Permission::LibraryRead)?;
        let root = self.root(root).await?;
        let entries = blocking(move || library_files::list(&root, &folder)).await?;
        Ok(entries
            .into_iter()
            .map(|e| files::Entry {
                name: e.name,
                directory: e.directory,
                size: e.size,
                modified_ms: e.modified_ms,
            })
            .collect())
    }

    async fn read(
        &mut self,
        root: u64,
        path: String,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, String> {
        self.may(Permission::LibraryRead)?;
        let root = self.root(root).await?;
        blocking(move || library_files::read(&root, &path, offset, length)).await
    }

    async fn write(
        &mut self,
        root: u64,
        path: String,
        contents: Vec<u8>,
        mode: files::WriteMode,
    ) -> Result<(), String> {
        let replace = matches!(mode, files::WriteMode::Replace);
        self.may(if replace { Permission::LibraryChange } else { Permission::LibraryAdd })?;
        let root = self.root(root).await?;
        let written =
            blocking(move || library_files::write(&root, &path, &contents, replace)).await?;
        self.written += 1;
        self.touched.push(written);
        Ok(())
    }

    async fn rename(&mut self, root: u64, from: String, to: String) -> Result<(), String> {
        self.may(Permission::LibraryChange)?;
        let root = self.root(root).await?;
        let (from, to) = blocking(move || library_files::rename(&root, &from, &to)).await?;
        self.touched.extend([from, to]);
        Ok(())
    }

    async fn delete(&mut self, root: u64, path: String) -> Result<(), String> {
        self.may(Permission::LibraryChange)?;
        let root = self.root(root).await?;
        let deleted = blocking(move || library_files::delete(&root, &path)).await?;
        self.touched.push(deleted);
        Ok(())
    }
}

fn check_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > MAX_KEY {
        return Err(format!("a key is 1 to {MAX_KEY} bytes"));
    }
    Ok(())
}

impl<L: Library> state::Host for Run<L> {
    async fn get(&mut self, key: String) -> Result<Option<Vec<u8>>, String> {
        check_key(&key)?;
        self.library.state_get(key).await
    }

    async fn set(&mut self, key: String, value: Vec<u8>) -> Result<(), String> {
        check_key(&key)?;
        if value.len() > MAX_VALUE {
            return Err(format!("a value is at most {} MB", MAX_VALUE >> 20));
        }
        self.library.state_set(key, value).await
    }

    async fn delete(&mut self, key: String) -> Result<(), String> {
        check_key(&key)?;
        self.library.state_delete(key).await
    }
}
