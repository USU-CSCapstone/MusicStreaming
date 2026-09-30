//! Where each library's files are: its active roots, and paths relative to them, which is how
//! the database stores every path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::db::libraries::Root;

/// Each library's active roots.
#[derive(Default)]
pub struct Roots(pub RwLock<HashMap<i64, Vec<Root>>>);

impl Roots {
    /// The root containing `path`, and the path relative to it.
    pub fn locate(&self, library: i64, path: &Path) -> Option<(i64, String)> {
        let roots = self.0.read().unwrap();
        let root = roots
            .get(&library)?
            .iter()
            .filter(|r| path.starts_with(&r.path))
            .max_by_key(|r| r.path.as_os_str().len())?;
        Some((root.id, relative(&root.path, path)))
    }

    pub fn root_by_path(&self, library: i64, root: &Path) -> Option<i64> {
        self.0
            .read()
            .unwrap()
            .get(&library)?
            .iter()
            .find(|r| r.path == root)
            .map(|r| r.id)
    }

    pub fn absolute(&self, library: i64, root_id: i64, rel: &str) -> Option<PathBuf> {
        let roots = self.0.read().unwrap();
        let root = roots.get(&library)?.iter().find(|r| r.id == root_id)?;
        Some(join(&root.path, rel))
    }
}

/// Relative path text with `/` separators; empty for the root itself.
pub fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

pub fn join(root: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    }
}
