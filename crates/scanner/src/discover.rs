//! Stage 1: walk a scope in a stable order, one directory at a time
//! (`design/scanning.md` §2, §9).
//!
//! Directories come out in sorted pre-order so a resume cursor is just "the
//! last directory finished". Each yielded directory carries its audio
//! candidates (extension-filtered, stat'ed) and its full filename list, which
//! the sidecar resolver reuses so no directory is listed twice.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use jewelcase_core::Format;

use crate::types::{Depth, Scope, system_time_ms};

/// An audio candidate: passed the extension filter, not yet opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: u64,
}

/// One directory's listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directory {
    pub path: PathBuf,
    pub candidates: Vec<Candidate>,
    /// Every regular file's name, sorted. For sidecar and lyrics lookup.
    pub files: BTreeSet<OsString>,
}

/// A directory that could not be listed. Reported as a problem on the
/// directory path; the walk continues.
#[derive(Debug)]
pub struct WalkError {
    pub path: PathBuf,
    pub error: io::Error,
}

pub enum WalkItem {
    Directory(Directory),
    Error(WalkError),
}

/// Compile exclude globs. Patterns are relative to the root
/// (`requirements/libraries.md` §1).
pub fn compile_excludes(patterns: &[String]) -> Result<GlobSet, globset::Error> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(Glob::new(p)?);
    }
    b.build()
}

/// Why a root is unusable right now (`requirements/scanning.md` §7).
#[derive(Debug)]
pub enum RootUnavailable {
    /// `stat` failed: unmounted, permission denied, gone.
    Inaccessible(io::Error),
    /// Listed fine but held nothing at all. Far more often an unmounted share
    /// than a deleted collection (`design/scanning.md` §8).
    Empty,
}

/// Confirm a root answers before touching anything under it.
pub fn check_root(root: &Path) -> Result<(), RootUnavailable> {
    let meta = fs::metadata(root).map_err(RootUnavailable::Inaccessible)?;
    if !meta.is_dir() {
        return Err(RootUnavailable::Inaccessible(io::Error::new(
            io::ErrorKind::NotADirectory,
            "root is not a directory",
        )));
    }
    let mut entries = fs::read_dir(root).map_err(RootUnavailable::Inaccessible)?;
    if entries.next().is_none() {
        return Err(RootUnavailable::Empty);
    }
    Ok(())
}

/// Walk `scope`, skipping directories at or before `resume_after` in
/// traversal order.
pub struct Walk<'a> {
    scope: &'a Scope,
    excludes: &'a GlobSet,
    resume_after: Option<&'a Path>,
    /// Directories still to visit, most recently pushed first (depth-first).
    stack: Vec<PathBuf>,
}

impl<'a> Walk<'a> {
    pub fn new(
        scope: &'a Scope,
        excludes: &'a GlobSet,
        resume_after: Option<&'a Path>,
    ) -> Walk<'a> {
        Walk {
            scope,
            excludes,
            resume_after,
            stack: vec![scope.path.clone()],
        }
    }

    fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.scope.root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf())
    }

    fn excluded(&self, path: &Path) -> bool {
        !self.excludes.is_empty() && self.excludes.is_match(self.relative(path))
    }

    /// Where `dir` sits relative to the cursor in traversal order.
    fn cursor_position(&self, dir: &Path) -> Position {
        let Some(cursor) = self.resume_after else {
            return Position::After;
        };
        // The cursor directory itself and its ancestors were persisted, but
        // their later children were not: descend without yielding.
        if cursor.starts_with(dir) {
            return Position::Ancestor;
        }
        match compare_components(dir, cursor) {
            Ordering::Less => Position::Done,
            _ => Position::After,
        }
    }
}

enum Position {
    /// Already persisted by the interrupted run; do not yield or descend.
    Done,
    /// The cursor directory or an ancestor of it: already persisted, but
    /// descend to reach children and siblings after it.
    Ancestor,
    /// Not yet visited.
    After,
}

/// Component-wise ordering, which is the order the walk visits siblings in.
fn compare_components(a: &Path, b: &Path) -> Ordering {
    a.components()
        .map(|c| c.as_os_str().to_owned())
        .cmp(b.components().map(|c| c.as_os_str().to_owned()))
}

impl Iterator for Walk<'_> {
    type Item = WalkItem;

    fn next(&mut self) -> Option<WalkItem> {
        loop {
            let dir = self.stack.pop()?;
            let position = self.cursor_position(&dir);
            if matches!(position, Position::Done) {
                continue;
            }
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(error) => return Some(WalkItem::Error(WalkError { path: dir, error })),
            };
            let mut subdirs = Vec::new();
            let mut files = BTreeSet::new();
            let mut candidates = Vec::new();
            for entry in entries {
                let Ok(entry) = entry else { continue };
                let path = entry.path();
                let Ok(meta) = entry.metadata() else { continue };
                let file_type = meta.file_type();
                if file_type.is_dir() {
                    if self.scope.depth == Depth::Subtree && !self.excluded(&path) {
                        subdirs.push(path);
                    }
                } else if file_type.is_file() || file_type.is_symlink() {
                    // Follow symlinked files by stat'ing the target.
                    let meta = if file_type.is_symlink() {
                        match fs::metadata(&path) {
                            Ok(m) if m.is_file() => m,
                            _ => continue,
                        }
                    } else {
                        meta
                    };
                    files.insert(entry.file_name());
                    if is_candidate(&path) && !self.excluded(&path) {
                        candidates.push(Candidate {
                            path,
                            size: meta.len(),
                            mtime_ms: meta.modified().map(system_time_ms).unwrap_or(0),
                        });
                    }
                }
            }
            // Depth-first in sorted order: push children reversed.
            subdirs.sort_by(|a, b| compare_components(a, b));
            for sub in subdirs.into_iter().rev() {
                self.stack.push(sub);
            }
            if matches!(position, Position::Ancestor) {
                continue;
            }
            candidates.sort_by(|a, b| a.path.cmp(&b.path));
            return Some(WalkItem::Directory(Directory {
                path: dir,
                candidates,
                files,
            }));
        }
    }
}

/// The extension filter (`design/scanning.md` §3): the only test a file
/// must pass to be opened.
pub fn is_candidate(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| Format::extension_is_candidate(&e.to_ascii_lowercase()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    fn dirs(scope: &Scope, excludes: &GlobSet, resume: Option<&Path>) -> Vec<PathBuf> {
        Walk::new(scope, excludes, resume)
            .filter_map(|i| match i {
                WalkItem::Directory(d) => Some(d.path),
                WalkItem::Error(_) => None,
            })
            .collect()
    }

    #[test]
    fn sorted_preorder_and_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for p in ["a/1.flac", "a/x/2.flac", "b/3.mp3", "c/4.ogg", "c/y/5.opus"] {
            touch(&root.join(p));
        }
        let scope = Scope::root(root);
        let none = compile_excludes(&[]).unwrap();
        let all = dirs(&scope, &none, None);
        assert_eq!(
            all,
            vec![
                root.to_path_buf(),
                root.join("a"),
                root.join("a/x"),
                root.join("b"),
                root.join("c"),
                root.join("c/y")
            ]
        );
        let resumed = dirs(&scope, &none, Some(&root.join("a/x")));
        assert_eq!(
            resumed,
            vec![root.join("b"), root.join("c"), root.join("c/y")]
        );
        let resumed = dirs(&scope, &none, Some(&root.join("c")));
        assert_eq!(resumed, vec![root.join("c/y")]);
    }

    #[test]
    fn excludes_and_extension_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for p in [
            "keep/1.flac",
            "keep/notes.txt",
            "keep/cover.jpg",
            "backup/2.flac",
            "keep/song.wma",
        ] {
            touch(&root.join(p));
        }
        let scope = Scope::root(root);
        let ex = compile_excludes(&["backup".into(), "backup/**".into()]).unwrap();
        let items: Vec<Directory> = Walk::new(&scope, &ex, None)
            .filter_map(|i| match i {
                WalkItem::Directory(d) => Some(d),
                _ => None,
            })
            .collect();
        assert_eq!(items.len(), 2);
        let keep = items.iter().find(|d| d.path == root.join("keep")).unwrap();
        assert_eq!(keep.candidates.len(), 1);
        assert!(keep.candidates[0].path.ends_with("1.flac"));
        assert!(keep.files.contains(&OsString::from("cover.jpg")));
    }

    #[test]
    fn directory_depth_does_not_descend() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("a/1.flac"));
        touch(&root.join("a/x/2.flac"));
        let scope = Scope::folder(root, root.join("a"), Depth::Directory);
        let none = compile_excludes(&[]).unwrap();
        assert_eq!(dirs(&scope, &none, None), vec![root.join("a")]);
    }

    #[test]
    fn root_checks() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(
            check_root(tmp.path()),
            Err(RootUnavailable::Empty)
        ));
        touch(&tmp.path().join("x.flac"));
        assert!(check_root(tmp.path()).is_ok());
        assert!(matches!(
            check_root(&tmp.path().join("nope")),
            Err(RootUnavailable::Inaccessible(_))
        ));
    }
}
