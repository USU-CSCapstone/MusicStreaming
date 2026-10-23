//! Stage 1: walk a scope in a stable order, one directory at a time
//! (`design/scanning.md` §2, §9).
//!
//! Directories come out in sorted pre-order so a resume cursor is just "the last directory
//! finished". Each yielded directory carries its audio candidates (extension-filtered, stat'ed),
//! its regular files with their sizes and modified times, which the sidecar resolver reuses so no
//! directory is listed or stat'ed twice, and its folders, against which reconcile finds the
//! folders that have gone.
//!
//! Symlinks are never followed ([`crate::confine`]), so the walk needs no loop
//! detection. Each link it lists is reported as a problem instead, unless excluded.
//!
//! A directory or entry the walk cannot read is reported too. A directory that cannot be listed
//! is not yielded, so reconcile leaves what is indexed in it alone, and an entry that cannot be
//! read is named in its directory's [`Directory::unreadable`]. One that no longer exists is gone
//! rather than unreadable, and lists as empty without a problem, so reconcile marks what was in
//! it missing.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use jewelcase_core::Format;

use crate::clock::system_time_ms;
use crate::confine::not_followed;
use crate::scope::{Depth, Scope};

/// An audio candidate: passed the extension filter, not yet opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: u64,
}

/// A regular file's size and modified time, as its directory's listing found them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStat {
    pub size: u64,
    pub mtime_ms: u64,
}

/// One directory's listing.
#[derive(Debug)]
pub struct Directory {
    pub path: PathBuf,
    /// Whether the directory was there to list. One deleted or replaced by a file, or the folder of
    /// a scope in an excluded folder, comes out empty and unlisted, so what is indexed in it goes
    /// missing.
    pub listed: bool,
    pub candidates: Vec<Candidate>,
    /// Every regular file's size and modified time, by name, sorted. For sidecar and lyrics
    /// lookup.
    pub files: BTreeMap<OsString, FileStat>,
    /// The name of each folder in it that the excludes leave in, sorted. A subtree walk descends
    /// into each.
    pub folders: BTreeSet<OsString>,
    /// The symlinks in it, which the walk does not follow.
    pub skipped_links: Vec<WalkError>,
    /// Its entries that could not be read. A failure partway through the listing is
    /// reported on the directory itself.
    pub unreadable: Vec<WalkError>,
}

/// A directory that could not be listed, an entry that could not be read, or a
/// symlink the walk does not follow. Reported as a problem on its path; the walk
/// continues.
#[derive(Debug)]
pub struct WalkError {
    pub path: PathBuf,
    pub error: io::Error,
}

/// What a walk yields: a directory's listing, or a directory it could not list.
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

/// Whether `path`, which lies under `root`, matches `excludes` itself. Whether it lies in an
/// excluded folder is [`excluded_scope`]'s question.
pub fn excluded(root: &Path, path: &Path, excludes: &GlobSet) -> bool {
    !excludes.is_empty() && excludes.is_match(path.strip_prefix(root).unwrap_or(path))
}

/// Whether `scope` lies in an excluded folder: its own folder, or one between it and its root,
/// matches `excludes`. Nothing in such a scope is indexed. The root itself is never matched, as
/// the walk never matches it.
pub fn excluded_scope(scope: &Scope, excludes: &GlobSet) -> bool {
    let Ok(relative) = scope.path.strip_prefix(&scope.root) else {
        return false;
    };
    relative.ancestors().any(|dir| !dir.as_os_str().is_empty() && excludes.is_match(dir))
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
    /// Whether the scope lies in an excluded folder ([`excluded_scope`]).
    scope_excluded: bool,
    /// Directories still to visit, most recently pushed first (depth-first).
    stack: Vec<PathBuf>,
}

impl<'a> Walk<'a> {
    /// A walk starting at `scope.path` that leaves out what `excludes` matches. A scope in an
    /// excluded folder ([`excluded_scope`]) lists its own folder as empty, so reconcile treats
    /// what is indexed in it as missing, as a walk of its root does.
    pub fn new(
        scope: &'a Scope,
        excludes: &'a GlobSet,
        resume_after: Option<&'a Path>,
    ) -> Walk<'a> {
        let scope_excluded = excluded_scope(scope, excludes);
        Walk { scope, excludes, resume_after, scope_excluded, stack: vec![scope.path.clone()] }
    }

    /// Whether `path` matches the excludes ([`excluded`]).
    fn excluded(&self, path: &Path) -> bool {
        excluded(&self.scope.root, path, self.excludes)
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

/// Where a directory sits relative to the resume cursor.
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

    /// The next directory in sorted pre-order, or the next one that could not be listed.
    fn next(&mut self) -> Option<WalkItem> {
        loop {
            let dir = self.stack.pop()?;
            let position = self.cursor_position(&dir);
            if matches!(position, Position::Done) {
                continue;
            }
            // A folder deleted or renamed away, or a scope's start that is a file, holds nothing,
            // as does a scope in an excluded folder.
            let entries = if self.scope_excluded {
                None
            } else {
                match fs::read_dir(&dir) {
                    Ok(entries) => Some(entries),
                    Err(error) if gone(&error) => None,
                    Err(error) => return Some(WalkItem::Error(WalkError { path: dir, error })),
                }
            };
            let listed = entries.is_some();
            let mut folders = BTreeSet::new();
            let mut files = BTreeMap::new();
            let mut candidates = Vec::new();
            let mut skipped_links = Vec::new();
            let mut unreadable = Vec::new();
            for entry in entries.into_iter().flatten() {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        unreadable.push(WalkError { path: dir.clone(), error });
                        continue;
                    }
                };
                let path = entry.path();
                let meta = match entry.metadata() {
                    Ok(meta) => meta,
                    Err(error) => {
                        if !self.excluded(&path) {
                            unreadable.push(WalkError { path, error });
                        }
                        continue;
                    }
                };
                let file_type = meta.file_type();
                if file_type.is_dir() {
                    if !self.excluded(&path) {
                        folders.insert(entry.file_name());
                    }
                } else if file_type.is_symlink() {
                    // Excluded content is never indexed, so its links are not reported.
                    if !self.excluded(&path) {
                        skipped_links.push(WalkError { path, error: not_followed() });
                    }
                } else if file_type.is_file() {
                    let mtime_ms = meta.modified().map(system_time_ms).unwrap_or(0);
                    let stat = FileStat { size: meta.len(), mtime_ms };
                    files.insert(entry.file_name(), stat);
                    if is_candidate(&path) && !self.excluded(&path) {
                        candidates.push(Candidate { path, size: stat.size, mtime_ms });
                    }
                }
            }
            unreadable.retain(|e| !gone(&e.error));
            // Depth-first in sorted order: push children reversed.
            if self.scope.depth == Depth::Subtree {
                self.stack.extend(folders.iter().rev().map(|name| dir.join(name)));
            }
            if matches!(position, Position::Ancestor) {
                continue;
            }
            candidates.sort_by(|a, b| a.path.cmp(&b.path));
            return Some(WalkItem::Directory(Directory {
                path: dir,
                listed,
                candidates,
                files,
                folders,
                skipped_links,
                unreadable,
            }));
        }
    }
}

/// Whether `error` says the path does not exist, which makes it gone rather than unreadable:
/// nothing is there, or a file stands where a folder on its way would be.
pub fn gone(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory)
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
    use std::ffi::OsStr;

    use super::*;

    /// Writes a one-byte file at `p`, making the folders above it.
    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    /// The path of each directory a walk of `scope` lists.
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
        assert_eq!(resumed, vec![root.join("b"), root.join("c"), root.join("c/y")]);
        let resumed = dirs(&scope, &none, Some(&root.join("c")));
        assert_eq!(resumed, vec![root.join("c/y")]);
    }

    #[test]
    fn excludes_and_extension_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for p in
            ["keep/1.flac", "keep/notes.txt", "keep/cover.jpg", "backup/2.flac", "keep/song.wma"]
        {
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
        assert!(keep.files.contains_key(OsStr::new("cover.jpg")));
    }

    /// Each directory `scope`'s walk yields, as its path, whether it was listed, and its
    /// candidates' and folders' names.
    fn shapes(
        scope: &Scope,
        excludes: &GlobSet,
    ) -> Vec<(PathBuf, bool, Vec<PathBuf>, Vec<OsString>)> {
        let listed = listings(scope, excludes).into_iter();
        listed
            .map(|d| {
                let candidates = d.candidates.iter().map(|c| c.path.clone()).collect();
                (d.path, d.listed, candidates, d.folders.into_iter().collect())
            })
            .collect()
    }

    /// A scope at or under a folder the excludes match lists its own folder as empty, even when
    /// no pattern matches the scope's own path, while a walk of the root lists the rest and
    /// leaves that folder out.
    #[test]
    fn a_scope_in_an_excluded_folder_lists_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("keep/1.flac"));
        touch(&root.join("backup/x/2.flac"));
        let ex = compile_excludes(&["backup".into()]).unwrap();
        let backup = |depth| Scope::folder(root, root.join("backup"), depth);
        let inside = |depth| Scope::folder(root, root.join("backup/x"), depth);
        for scope in [backup(Depth::Subtree), inside(Depth::Subtree), inside(Depth::Directory)] {
            assert_eq!(shapes(&scope, &ex), [(scope.path.clone(), false, vec![], vec![])]);
        }
        assert_eq!(dirs(&Scope::root(root), &ex, None), [root.to_path_buf(), root.join("keep")]);
        assert_eq!(shapes(&Scope::root(root), &ex)[0].3, ["keep"], "backup is not a folder of it");
        // The root's own relative path is empty, and the walk never matches it.
        let empty = compile_excludes(&["".into()]).unwrap();
        assert_eq!(dirs(&backup(Depth::Directory), &empty, None), vec![root.join("backup")]);
    }

    /// A Directory walk names its folder's folders without descending into them.
    #[test]
    fn directory_depth_does_not_descend() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("a/1.flac"));
        touch(&root.join("a/x/2.flac"));
        let scope = Scope::folder(root, root.join("a"), Depth::Directory);
        let none = compile_excludes(&[]).unwrap();
        let a = (root.join("a"), true, vec![root.join("a/1.flac")], vec![OsString::from("x")]);
        assert_eq!(shapes(&scope, &none), [a]);
    }

    /// A folder that is gone, or a file where a scope's folder was, lists as empty without an
    /// error.
    #[test]
    fn a_gone_folder_lists_as_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("file.flac"));
        let none = compile_excludes(&[]).unwrap();
        for path in [root.join("gone"), root.join("file.flac")] {
            let scope = Scope::folder(root, &path, Depth::Subtree);
            let items: Vec<WalkItem> = Walk::new(&scope, &none, None).collect();
            assert!(!items.iter().any(|i| matches!(i, WalkItem::Error(_))), "{path:?}");
            assert_eq!(shapes(&scope, &none), [(path, false, vec![], vec![])]);
        }
    }

    /// Every directory a walk of `scope` lists.
    fn listings(scope: &Scope, excludes: &GlobSet) -> Vec<Directory> {
        Walk::new(scope, excludes, None)
            .filter_map(|i| match i {
                WalkItem::Directory(d) => Some(d),
                WalkItem::Error(_) => None,
            })
            .collect()
    }

    /// The links each listed directory skipped, sorted.
    fn skipped(listed: &[Directory]) -> Vec<Vec<PathBuf>> {
        listed
            .iter()
            .map(|d| {
                let mut paths: Vec<PathBuf> =
                    d.skipped_links.iter().map(|s| s.path.clone()).collect();
                paths.sort();
                paths
            })
            .collect()
    }

    #[test]
    fn links_are_skipped() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let (music, more) = (tmp.path().join("music"), tmp.path().join("more"));
        touch(&music.join("A/01.flac"));
        touch(&more.join("B/02.flac"));
        touch(&tmp.path().join("other/C/03.flac"));
        touch(&tmp.path().join("secret.txt"));
        // Within the root, to another root of the library, to another library, out of every
        // library, and a loop back to the root.
        symlink(music.join("A/01.flac"), music.join("A/same-root.flac")).unwrap();
        symlink(more.join("B/02.flac"), music.join("A/other-root.flac")).unwrap();
        symlink(tmp.path().join("other/C/03.flac"), music.join("A/other-library.flac")).unwrap();
        symlink(tmp.path().join("secret.txt"), music.join("A/01.lrc")).unwrap();
        symlink(&music, music.join("loop")).unwrap();

        let listed = listings(&Scope::root(&music), &compile_excludes(&[]).unwrap());
        let paths: Vec<&Path> = listed.iter().map(|d| d.path.as_path()).collect();
        assert_eq!(paths, [music.as_path(), &music.join("A")], "the loop is not descended");
        assert_eq!(listed[1].candidates.len(), 1);
        let files: Vec<&OsString> = listed[1].files.keys().collect();
        assert_eq!(files, [OsStr::new("01.flac")]);
        let a = |name| music.join("A").join(name);
        assert_eq!(
            skipped(&listed),
            [
                vec![music.join("loop")],
                vec![
                    a("01.lrc"),
                    a("other-library.flac"),
                    a("other-root.flac"),
                    a("same-root.flac")
                ]
            ]
        );
    }

    #[test]
    fn a_root_reached_through_a_link_is_walked() {
        let tmp = tempfile::tempdir().unwrap();
        let (real, music) = (tmp.path().join("real"), tmp.path().join("music"));
        touch(&real.join("A/01.flac"));
        std::os::unix::fs::symlink(&real, &music).unwrap();
        let listed = listings(&Scope::root(&music), &compile_excludes(&[]).unwrap());
        assert_eq!(listed[1].candidates[0].path, music.join("A/01.flac"));
    }

    #[test]
    fn an_excluded_link_is_not_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("music");
        touch(&root.join("a/1.flac"));
        touch(&tmp.path().join("secret.flac"));
        std::os::unix::fs::symlink(tmp.path().join("secret.flac"), root.join("a/skip.flac"))
            .unwrap();
        let listed =
            listings(&Scope::root(&root), &compile_excludes(&["a/skip.flac".into()]).unwrap());
        assert!(skipped(&listed).iter().all(Vec::is_empty));
    }

    #[test]
    fn root_checks() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(matches!(check_root(tmp.path()), Err(RootUnavailable::Empty)));
        touch(&tmp.path().join("x.flac"));
        assert!(check_root(tmp.path()).is_ok());
        assert!(matches!(
            check_root(&tmp.path().join("nope")),
            Err(RootUnavailable::Inaccessible(_))
        ));
    }
}
