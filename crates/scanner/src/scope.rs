//! Scopes, the units of scanning work: what triggers enqueue, the queue coalesces and a
//! scan walks (`design/scanning.md` §9).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How deep a scope reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Depth {
    /// Only the files directly in the directory. What the watcher enqueues
    /// for an ordinary file change.
    Directory,
    /// The directory and everything below it.
    Subtree,
}

/// A unit of scanning work: a root, or a folder inside one
/// (`design/scanning.md` §9).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    pub root: PathBuf,
    /// Absolute path, equal to or under `root`.
    pub path: PathBuf,
    pub depth: Depth,
}

impl Scope {
    /// The whole of `root`, with everything below it.
    pub fn root(root: impl Into<PathBuf>) -> Scope {
        let root = root.into();
        Scope { path: root.clone(), root, depth: Depth::Subtree }
    }

    /// The folder `path` inside `root`, to `depth`. `path` is absolute and equal to or under
    /// `root`.
    pub fn folder(root: impl Into<PathBuf>, path: impl Into<PathBuf>, depth: Depth) -> Scope {
        Scope { root: root.into(), path: path.into(), depth }
    }

    /// Whether every file `other` visits is also visited by `self`.
    pub fn covers(&self, other: &Scope) -> bool {
        if self.root != other.root {
            return false;
        }
        match self.depth {
            Depth::Subtree => other.path.starts_with(&self.path),
            Depth::Directory => other.depth == Depth::Directory && other.path == self.path,
        }
    }

    /// Whether `path` is a file this scope visits: one under its folder, or for a Directory
    /// scope directly in it. The folder itself is not one.
    pub fn contains_file(&self, path: &Path) -> bool {
        match self.depth {
            Depth::Subtree => path != self.path && path.starts_with(&self.path),
            Depth::Directory => path.parent() == Some(self.path.as_path()),
        }
    }
}
