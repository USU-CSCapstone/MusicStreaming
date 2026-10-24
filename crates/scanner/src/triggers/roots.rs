//! The watcher's roots (`design/scanning.md` §9): a watch of each root, set up afresh when the
//! directory at its path changes.

mod events;
mod watch;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use globset::GlobSet;
use notify::ErrorKind;
use tracing::{info, warn};

use super::Timing;
use crate::queue::Scanner;
use crate::scan::Trigger;
use crate::scope::Scope;
use crate::worker::Stop;
pub(super) use watch::RootWatch;

/// Where every root's watch sends its events.
pub(super) type Events = Sender<notify::Result<notify::Event>>;

/// A root of the library and its watch.
struct Root {
    path: PathBuf,
    watch: RootWatch,
}

/// Every root of one library with its watch. Dropping it ends the watches.
pub(super) struct Roots {
    scanner: Scanner,
    events: Events,
    /// What the scan leaves out, which is not watched either.
    excludes: Arc<GlobSet>,
    roots: Vec<Root>,
    check: Duration,
    next_check: Instant,
}

impl Roots {
    /// Watches each of `paths` in turn, leaving out what `excludes` match and logging how much
    /// of each is covered. Stops walking once `stop` is asked.
    pub(super) fn start(
        scanner: Scanner,
        paths: &[PathBuf],
        events: Events,
        excludes: Arc<GlobSet>,
        timing: &Timing,
        stop: &Stop,
    ) -> Roots {
        let roots = paths
            .iter()
            .map(|path| {
                let watch = RootWatch::start(path, &excludes, &events, stop);
                report(path, &watch);
                Root { path: path.clone(), watch }
            })
            .collect();
        let next_check = Instant::now() + timing.check;
        Roots { scanner, events, excludes, roots, check: timing.check, next_check }
    }

    /// At most once per `check`, watches afresh each root whose directory, by device and
    /// inode, is no longer the one it watches, as when a share is mounted over it or unmounted
    /// from under it, and queues a scan of it unless it cannot be read, since what changed in it
    /// unwatched was missed. A root that cannot be read has no identity, so it is watched
    /// afresh and scanned once it can be. Stops walking once `stop` is asked.
    pub(super) fn check(&mut self, now: Instant, stop: &Stop) {
        if now < self.next_check {
            return;
        }
        self.next_check = now + self.check;
        for root in &mut self.roots {
            if identity(&root.path) == root.watch.identity {
                continue;
            }
            // Ends the old OS watches first, so they stop counting against the OS limit.
            // Dropping the watcher ends them on notify's own thread, a moment after this.
            root.watch.watcher = None;
            root.watch = RootWatch::start(&root.path, &self.excludes, &self.events, stop);
            report(&root.path, &root.watch);
            if root.watch.identity.is_some() {
                self.scanner.request(Trigger::Watch, vec![Scope::root(root.path.clone())]);
            }
        }
    }
}

/// Whether `error` says the OS limit on watches was reached.
fn limit(error: &notify::Error) -> bool {
    matches!(error.kind, ErrorKind::MaxFilesWatch)
}

/// Logs how much of `root` `watch` covers.
fn report(root: &Path, watch: &RootWatch) {
    let root = root.display();
    match &watch.coverage {
        Ok(()) => info!(%root, "watching every folder"),
        Err(error) if limit(error) => warn!(
            %root,
            "part of the root is not watched: the OS limit on watches was reached; \
             raise fs.inotify.max_user_watches"
        ),
        Err(error) => warn!(%root, %error, "cannot watch all of the root"),
    }
}

/// The directory at `root`, as device and inode, so a share mounted over it or unmounted from
/// under it reads as another; `None` when it cannot be read.
fn identity(root: &Path) -> Option<(u64, u64)> {
    fs::metadata(root).map(|meta| (meta.dev(), meta.ino())).ok()
}

#[cfg(test)]
mod tests;
