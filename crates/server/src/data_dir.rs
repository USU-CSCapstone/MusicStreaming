//! The data directory: the only place Jewelcase writes (design/general.md §2).
//!
//! ```text
//! data/
//! ├── jewelcase.lock  # held while a server runs
//! ├── state/          # database, custom artwork, plugin data — back this up
//! └── cache/          # transcodes, resized images, indexes — rebuilt on demand
//! ```

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

pub struct DataDir {
    /// The root of the data directory
    root: PathBuf,
    /// Lock file to prevent multiple servers from using the same data directory
    _lock: File,
}

impl DataDir {
    /// Create data dir layout and lock it for the server's lifetime
    pub fn open(root: &Path) -> anyhow::Result<DataDir> {
        fs::create_dir_all(root)
            .with_context(|| format!("cannot create the data directory {}", root.display()))?;
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot resolve the data directory {}", root.display()))?;

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
        for dir in [data_dir.state(), data_dir.cache()] {
            fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        }
        Ok(data_dir)
    }

    /// The root of the data directory
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Important state that must be preserved
    pub fn state(&self) -> PathBuf {
        self.root.join("state")
    }

    /// Rebuildable state that can be deleted and reconstructed
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_the_layout() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("data");
        let data_dir = DataDir::open(&root).unwrap();
        assert!(data_dir.state().is_dir());
        assert!(data_dir.cache().is_dir());
    }

    #[test]
    fn a_second_server_cannot_share_the_directory() {
        let temp = tempfile::tempdir().unwrap();
        let first = DataDir::open(temp.path()).unwrap();
        let error = DataDir::open(temp.path()).err().unwrap();
        assert!(error.to_string().contains("already using"), "{error:#}");

        drop(first);
        DataDir::open(temp.path()).unwrap();
    }
}
