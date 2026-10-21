//! One root's watch (`design/scanning.md` §9): each folder the scan walks, watched on its own,
//! so one that cannot be watched leaves the rest watched.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::GlobSet;
use notify::{ErrorKind, RecursiveMode, Watcher};

use super::{Events, identity, limit};
use crate::confine::without_links;
use crate::discover::{excluded, excluded_scope, gone};
use crate::scope::{Depth, Scope};
use crate::triggers::watcher;

/// One root's watch, as it was set up.
pub(in crate::triggers) struct RootWatch {
    /// Holds the OS watches until dropped; `None` when no watcher could be made.
    pub(super) watcher: Option<Box<dyn Watcher>>,
    /// The directory watched, as device and inode, read before the watch was set up.
    pub(super) identity: Option<(u64, u64)>,
    /// Whether the watch covers the whole root, or why it may not.
    pub(super) coverage: notify::Result<()>,
    /// What the scan leaves out, which is not watched either.
    pub(super) excludes: Arc<GlobSet>,
    /// The folders passed over because they could not be read, tried again once a change of
    /// permissions may let them be. Usually empty, or a root-owned `lost+found`.
    pub(super) unreadable: BTreeSet<PathBuf>,
}

impl RootWatch {
    /// Watches `root` and each folder below it that the scan walks, leaving out what `excludes`
    /// match ([`watch_folders`]), and sends the events to `events`.
    pub(in crate::triggers) fn start(
        root: &Path,
        excludes: &Arc<GlobSet>,
        events: &Events,
    ) -> RootWatch {
        match watcher(events.clone()) {
            Ok(watcher) => RootWatch::with(Box::new(watcher), root, excludes),
            Err(error) => RootWatch {
                watcher: None,
                identity: identity(root),
                coverage: Err(error),
                excludes: excludes.clone(),
                unreadable: BTreeSet::new(),
            },
        }
    }

    /// [`RootWatch::start`] with `watcher`, which tests replace to control what watching each
    /// folder meets.
    pub(super) fn with(
        mut watcher: Box<dyn Watcher>,
        root: &Path,
        excludes: &Arc<GlobSet>,
    ) -> RootWatch {
        let identity = identity(root);
        let mut unreadable = BTreeSet::new();
        let coverage = watch_folders(&Scope::root(root), excludes, &mut unreadable, |folder| {
            watcher.watch(folder, RecursiveMode::NonRecursive)
        });
        let excludes = excludes.clone();
        RootWatch { watcher: Some(watcher), identity, coverage, excludes, unreadable }
    }

    /// Watches `folder`, which lies in `root`, and each folder below it that the scan walks, as
    /// [`RootWatch::start`] does the root's. Anything but a folder the scan walks is left alone:
    /// a file, a link or a path through one, and what cannot be read, which the scan reports.
    /// Of the folders at or below `folder` passed over as unreadable, only those the walk still
    /// cannot read are kept. Without a watcher this does nothing.
    pub(super) fn watch_folder(&mut self, root: &Path, folder: &Path) -> notify::Result<()> {
        let Some(watcher) = &mut self.watcher else {
            return Ok(());
        };
        self.unreadable.retain(|unread| !unread.starts_with(folder));
        if !without_links(root, folder).is_ok_and(|real| real.is_dir()) {
            return Ok(());
        }
        let scope = Scope::folder(root, folder, Depth::Subtree);
        watch_folders(&scope, &self.excludes, &mut self.unreadable, |folder| {
            watcher.watch(folder, RecursiveMode::NonRecursive)
        })
    }

    /// The folders at or below `folder` passed over because they could not be read, which a
    /// change to `folder`'s permissions may have made readable: itself, or, as when it gains
    /// the permission to search it, those in it.
    pub(super) fn unreadable_under(&self, folder: &Path) -> Vec<PathBuf> {
        self.unreadable.iter().filter(|unread| unread.starts_with(folder)).cloned().collect()
    }
}

/// Watches, with `add`, the folder of `scope` and every folder below it that the scan walks,
/// each before it is listed, so a folder made in it meanwhile is still seen. What the scan
/// leaves out is not watched: an excluded folder, a link, and below the root, a folder that is
/// gone or cannot be read, which goes into `unreadable`, as the scan reports it. Any other
/// failure leaves that folder and those below it unwatched, and the first is returned once the
/// walk ends, except the OS limit on watches, which ends the walk at once, since every later
/// folder would meet it too. The root is never passed over: it must be watched.
fn watch_folders(
    scope: &Scope,
    excludes: &GlobSet,
    unreadable: &mut BTreeSet<PathBuf>,
    mut add: impl FnMut(&Path) -> notify::Result<()>,
) -> notify::Result<()> {
    let mut covered = Ok(());
    let mut stack =
        if excluded_scope(scope, excludes) { Vec::new() } else { vec![scope.path.clone()] };
    while let Some(folder) = stack.pop() {
        let listed = add(&folder).and_then(|()| {
            folders_in(&scope.root, &folder, excludes)
                .map_err(|error| notify::Error::io(error).add_path(folder.clone()))
        });
        match listed {
            Ok(folders) => stack.extend(folders),
            Err(error) if limit(&error) => return Err(error),
            Err(error) if vanished(&error) && folder != scope.root => {}
            // Only a folder the scan cannot read either is passed over; one refused just the
            // watch, as a security policy may do, leaves the root not wholly watched.
            Err(error)
                if denied(&error) && folder != scope.root && fs::read_dir(&folder).is_err() =>
            {
                unreadable.insert(folder);
            }
            Err(error) => {
                if covered.is_ok() {
                    covered = Err(error);
                }
            }
        }
    }
    covered
}

/// The folders in `folder`, which lies under `root`, that the scan walks: neither links nor
/// excluded. An entry whose type cannot be read is not one, as the scan does not descend it.
fn folders_in(root: &Path, folder: &Path, excludes: &GlobSet) -> io::Result<Vec<PathBuf>> {
    let mut folders = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) && !excluded(root, &path, excludes) {
            folders.push(path);
        }
    }
    Ok(folders)
}

/// Whether `error` says the folder is gone.
fn vanished(error: &notify::Error) -> bool {
    match &error.kind {
        ErrorKind::PathNotFound => true,
        ErrorKind::Io(error) => gone(error),
        _ => false,
    }
}

/// Whether `error` says this user may not read or watch the folder.
fn denied(error: &notify::Error) -> bool {
    matches!(&error.kind, ErrorKind::Io(error) if error.kind() == io::ErrorKind::PermissionDenied)
}

#[cfg(test)]
mod tests;
