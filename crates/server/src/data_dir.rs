//! The data directory: the only place Jewelcase writes (design/general.md §2).
//!
//! ```text
//! data/
//! ├── jewelcase.lock  # held while a server runs
//! ├── state/          # the database and installed plugins — back this up
//! └── cache/          # resized images and compiled plugins — rebuilt on demand
//! ```
//!
//! No library's root may be the data directory, inside it, or around it.

use std::fs::{self, DirBuilder, File, OpenOptions, TryLockError};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

use crate::db::libraries::overlap;

/// The data directory, locked against other servers for as long as this lives.
pub struct DataDir {
    /// The data directory itself, canonical.
    root: PathBuf,
    /// `jewelcase.lock`, held so a second server on the same directory fails to start.
    _lock: File,
}

impl DataDir {
    /// Creates the layout at `root` and locks it for as long as the `DataDir` lives. `music`, the
    /// canonical root the first library would have, is checked with [`apart`] before anything is
    /// written inside the data directory. Fails if the directory cannot be created or locked, or
    /// another server holds it.
    pub fn open(root: &Path, music: Option<&Path>) -> anyhow::Result<DataDir> {
        fs::create_dir_all(root)
            .with_context(|| format!("cannot create the data directory {}", root.display()))?;
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot resolve the data directory {}", root.display()))?;
        if let Some(music) = music {
            apart(&root, music)?;
        }

        let lock_path = root.join("jewelcase.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("cannot write to the data directory {}", root.display()))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => bail!(
                "another Jewelcase server is already using the data directory {}",
                root.display()
            ),
            Err(TryLockError::Error(error)) => {
                return Err(error).with_context(|| format!("cannot lock {}", lock_path.display()));
            }
        }

        let data_dir = DataDir { root, _lock: lock };
        // The database and plugin credentials are for the server's user alone, and so is the
        // cache, which holds compiled plugin code the server runs ([`DataDir::private_cache`]).
        // One that exists keeps the mode its owner gave it.
        for dir in [data_dir.state(), data_dir.cache()] {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        Ok(data_dir)
    }

    /// The data directory itself, canonical.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `state/`: what must be kept, such as the database and the installed plugins.
    pub fn state(&self) -> PathBuf {
        self.root.join("state")
    }

    /// `cache/`: what the server rebuilds if it is deleted.
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// `cache/<name>`, created for the server's user alone (0700), for code the server will
    /// load and run. wasmtime runs an entry in its cache of compiled plugins as native code once
    /// it sees that its own version and the engine's settings built it, with no check of who
    /// wrote it, so whoever can write there can run code as the server.
    ///
    /// Fails if the folder cannot be created, or if it or any folder above it up to the data
    /// directory is a symbolic link (whose target and the folders above that go unchecked),
    /// belongs to someone other than the data directory's owner, or can be written by group or
    /// others. A folder that exists keeps whatever mode it has, so it is checked rather than
    /// trusted to be 0700.
    pub fn private_cache(&self, name: &str) -> anyhow::Result<PathBuf> {
        let dir = self.cache().join(name);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
        let read = |folder: &Path| {
            fs::symlink_metadata(folder)
                .with_context(|| format!("cannot read the owner and mode of {}", folder.display()))
        };
        let owner = read(&self.root)?.uid();
        for folder in dir.ancestors().take_while(|folder| folder.starts_with(&self.root)) {
            let metadata = read(folder)?;
            let problem = if metadata.file_type().is_symlink() {
                "is a symbolic link"
            } else if metadata.uid() != owner {
                "belongs to another user"
            } else if metadata.permissions().mode() & 0o022 != 0 {
                "can be written by other users"
            } else {
                continue;
            };
            bail!("{} {problem}", folder.display());
        }
        Ok(dir)
    }
}

/// Fails when `root`, a library's canonical root, is `data`, the canonical data directory, or is
/// inside it or holds it, since Jewelcase would then write into the music
/// (`requirements/general.md` §3.1).
pub fn apart(data: &Path, root: &Path) -> anyhow::Result<()> {
    if overlap(data, root) {
        bail!(
            "the data directory {} and the music at {} overlap; each must be outside the other",
            data.display(),
            root.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_the_layout() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        let data_dir = DataDir::open(&root, None).unwrap();
        assert!(data_dir.state().is_dir());
        assert!(data_dir.cache().is_dir());
    }

    #[test]
    fn state_and_cache_are_the_owners_alone() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(&temp.path().join("data"), None).unwrap();
        for dir in [data_dir.state(), data_dir.cache(), data_dir.private_cache("code").unwrap()] {
            let mode = fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{}: {mode:o}", dir.display());
        }
    }

    /// Whichever folder from the data directory down others can write to, a private cache is
    /// refused, naming it.
    #[test]
    fn a_private_cache_others_can_write_to_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(temp.path(), None).unwrap();
        let code = data_dir.private_cache("code").unwrap();
        for (folder, mode) in [(&code, 0o777), (&data_dir.cache(), 0o770), (&data_dir.root, 0o757)]
        {
            fs::set_permissions(folder, fs::Permissions::from_mode(mode)).unwrap();
            let error = data_dir.private_cache("code").unwrap_err().to_string();
            assert_eq!(error, format!("{} can be written by other users", folder.display()));
            fs::set_permissions(folder, fs::Permissions::from_mode(0o700)).unwrap();
        }
        data_dir.private_cache("code").unwrap();
    }

    /// A private cache that is a symbolic link is refused, even one leading to a folder that is
    /// the owner's alone. A folder that belongs to another user is refused too, which needs root
    /// to set up, so it is not tested here.
    #[test]
    fn a_private_cache_through_a_symbolic_link_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(&temp.path().join("data"), None).unwrap();
        let elsewhere = temp.path().join("elsewhere");
        DirBuilder::new().mode(0o700).create(&elsewhere).unwrap();
        let link = data_dir.cache().join("code");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        let error = data_dir.private_cache("code").unwrap_err().to_string();
        assert_eq!(error, format!("{} is a symbolic link", link.display()));
    }

    #[test]
    fn an_existing_state_directory_keeps_its_mode() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o750)).unwrap();
        DataDir::open(temp.path(), None).unwrap();
        let mode = fs::metadata(&state).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o750, "{mode:o}");
    }

    #[test]
    fn music_must_be_apart_from_it() {
        let data = Path::new("/srv/data");
        for root in [data.to_owned(), data.join("music"), PathBuf::from("/srv")] {
            let error = apart(data, &root).unwrap_err();
            assert!(error.to_string().contains("overlap"), "{}: {error:#}", root.display());
        }
        apart(data, Path::new("/srv/music")).unwrap();
    }

    #[test]
    fn inside_the_music_nothing_is_written_but_the_directory_itself() {
        let temp = tempfile::tempdir().unwrap();
        let music = temp.path().canonicalize().unwrap();
        let error = DataDir::open(&music.join("data"), Some(&music)).err().unwrap();
        assert!(error.to_string().contains("overlap"), "{error:#}");
        assert_eq!(fs::read_dir(music.join("data")).unwrap().count(), 0);
        DataDir::open(&music.join("data"), Some(&music.join("elsewhere"))).unwrap();
    }

    #[test]
    fn a_second_server_cannot_share_the_directory() {
        let temp = tempfile::tempdir().unwrap();
        let first = DataDir::open(temp.path(), None).unwrap();
        let error = DataDir::open(temp.path(), None).err().unwrap();
        assert!(error.to_string().contains("already using"), "{error:#}");

        drop(first);
        DataDir::open(temp.path(), None).unwrap();
    }
}
