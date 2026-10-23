//! Where each library's files are: its active roots, and paths relative to them, which is how
//! the database stores every path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::db::libraries::Root;
use crate::lock::Recover;

/// Each library's active roots.
#[derive(Default)]
pub struct Roots(pub RwLock<HashMap<i64, Vec<Root>>>);

impl Roots {
    /// The root containing `path`, and the path relative to it.
    pub fn locate(&self, library: i64, path: &Path) -> Option<(i64, String)> {
        let roots = self.0.read().recover();
        let root = roots
            .get(&library)?
            .iter()
            .filter(|r| path.starts_with(&r.path))
            .max_by_key(|r| r.path.as_os_str().len())?;
        Some((root.id, relative(&root.path, path)))
    }

    /// The ID of `library`'s active root at `root`, if it is one.
    pub fn root_by_path(&self, library: i64, root: &Path) -> Option<i64> {
        self.0.read().recover().get(&library)?.iter().find(|r| r.path == root).map(|r| r.id)
    }

    /// The absolute path of `rel` under root `root_id` of `library`, if it is one of its active
    /// roots. Symlinks are not checked.
    pub fn absolute(&self, library: i64, root_id: i64, rel: &str) -> Option<PathBuf> {
        Some(join(&self.root(library, root_id)?, rel))
    }

    /// The path of root `root_id` of `library`, if it is one of its active roots.
    pub fn root(&self, library: i64, root_id: i64) -> Option<PathBuf> {
        let roots = self.0.read().recover();
        roots.get(&library)?.iter().find(|r| r.id == root_id).map(|r| r.path.clone())
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

/// The bounds of the relative paths under the folder `rel`, for `path >= lower AND path < upper`,
/// or `None` for the root, under which every path lies. Under BINARY collation the paths that
/// start with `{rel}/` are exactly those from it up to `{rel}0`, `0` being the character after
/// `/`: a range on the `(root_id, path)` indexes, with no pattern characters in folder names to
/// widen or narrow it.
pub fn bounds_under(rel: &str) -> Option<[String; 2]> {
    (!rel.is_empty()).then(|| [format!("{rel}/"), format!("{rel}0")])
}

/// `rel`, relative path text, under `root`; the root itself when `rel` is empty.
pub fn join(root: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() { root.to_path_buf() } else { root.join(rel) }
}
