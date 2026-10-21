//! The watcher's roots (`design/scanning.md` §9): a watch of each root, set up afresh when the
//! root is lost, and the fallback schedule while any root is not wholly watched.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use notify::{ErrorKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use tracing::{debug, info, warn};

use super::{Schedule, Timing, watcher};
use crate::queue::Scanner;
use crate::types::{Scope, Trigger};

/// Where every root's watch sends its events.
pub(super) type Events = Sender<notify::Result<notify::Event>>;

/// The longest wait between fresh watches of a root.
const MAX_BACKOFF: Duration = Duration::from_secs(60 * 60);

/// One root's watch, as it was set up.
pub(super) struct RootWatch {
    /// Holds the OS watch until dropped; `None` when no watcher could be made.
    watcher: Option<RecommendedWatcher>,
    /// The directory watched, as device and inode, read before the watch was set up.
    identity: Option<(u64, u64)>,
    /// Whether the watch covers the whole root, or why it may not.
    coverage: notify::Result<()>,
}

impl RootWatch {
    /// Watches `root` and every folder below it, sending the events to `events`. A watch that
    /// fails partway keeps the folders it reached.
    pub(super) fn start(root: &Path, events: &Events) -> RootWatch {
        let identity = identity(root);
        match watcher(events.clone()) {
            Ok(mut watcher) => {
                let coverage = watcher.watch(root, RecursiveMode::Recursive);
                RootWatch { watcher: Some(watcher), identity, coverage }
            }
            Err(error) => RootWatch { watcher: None, identity, coverage: Err(error) },
        }
    }
}

/// A root of the library and its watch.
struct Root {
    path: PathBuf,
    watch: RootWatch,
    /// The earliest the root is watched afresh.
    retry_at: Instant,
    /// When a check last found the root lost, or, until one has, when it was first watched.
    lost_at: Instant,
    /// The wait after the next fresh watch.
    backoff: Duration,
}

impl Root {
    /// Whether the root needs watching afresh: its watch failed short of the OS limit on
    /// watches, or the directory at its path is not the one watched.
    fn lost(&self) -> bool {
        matches!(&self.watch.coverage, Err(e) if !limit(e))
            || identity(&self.path) != self.watch.identity
    }

    /// Logs how much of the root its watch covers, and holds off its next fresh watch for the
    /// backoff, which then doubles, up to [`MAX_BACKOFF`].
    fn settle(&mut self, now: Instant) {
        report(&self.path, &self.watch);
        self.retry_at = now + self.backoff;
        self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
    }

    /// Ends the root's watch. Unwatching removes the OS watches before this returns, which
    /// dropping the watcher alone does not, so they no longer count against the OS limit when
    /// the root is watched afresh.
    fn unwatch(&mut self) {
        let Some(mut watcher) = self.watch.watcher.take() else {
            return;
        };
        // After an unmount the OS has already removed the watches, so this fails harmlessly.
        if let Err(error) = watcher.unwatch(&self.path) {
            debug!(root = %self.path.display(), %error, "the old watch was already gone");
        }
    }
}

/// Every root of one library with its watch, and the fallback schedule while they are not all
/// wholly watched.
pub(super) struct Roots<W> {
    scanner: Scanner,
    events: Events,
    watch: W,
    roots: Vec<Root>,
    check: Duration,
    next_check: Instant,
    /// How often the library is scanned while not wholly watched; `None` when it has a schedule.
    fallback_every: Option<Duration>,
    fallback: Option<Schedule>,
}

impl<W: Fn(&Path, &Events) -> RootWatch> Roots<W> {
    /// Watches each of `paths` in turn with `watch`, logging how much of each is covered, and
    /// starts the fallback schedule if any is not wholly watched.
    pub(super) fn start(
        scanner: Scanner,
        paths: &[PathBuf],
        events: Events,
        watch: W,
        timing: &Timing,
    ) -> Roots<W> {
        let now = Instant::now();
        let roots = paths
            .iter()
            .map(|path| {
                let watch = watch(path, &events);
                let path = path.clone();
                let mut root =
                    Root { path, watch, retry_at: now, lost_at: now, backoff: timing.check };
                root.settle(now);
                root
            })
            .collect();
        let mut roots = Roots {
            scanner,
            events,
            watch,
            roots,
            check: timing.check,
            next_check: now + timing.check,
            fallback_every: timing.fallback,
            fallback: None,
        };
        roots.fall_back();
        roots
    }

    /// At most once per `check`, watches afresh each root that is lost ([`Root::lost`]) and
    /// whose backoff has passed, and queues a scan of each one then wholly watched, since what
    /// changed in it unwatched was missed. A root found as it was watched, and not lost for a
    /// whole backoff, has its backoff reset.
    pub(super) fn check(&mut self, now: Instant) {
        if now < self.next_check {
            return;
        }
        self.next_check = now + self.check;
        for root in &mut self.roots {
            if !root.lost() {
                if now >= root.lost_at + root.backoff {
                    root.backoff = self.check;
                }
                continue;
            }
            root.lost_at = now;
            if now < root.retry_at {
                continue;
            }
            root.unwatch();
            root.watch = (self.watch)(&root.path, &self.events);
            root.settle(now);
            if root.watch.coverage.is_ok() {
                self.scanner.request(Trigger::Watch, vec![Scope::root(root.path.clone())]);
            }
        }
        self.fall_back();
    }

    /// Takes an error from a watch. When it says the OS limit on watches kept a folder in a
    /// root from being watched, as happens to one created after the limit was reached, that
    /// root counts as not wholly watched, and this returns true.
    pub(super) fn limit_reached(&mut self, error: &notify::Error) -> bool {
        if !limit(error) {
            return false;
        }
        let in_root =
            |root: &&mut Root| error.paths.iter().any(|path| path.starts_with(&root.path));
        let Some(root) = self.roots.iter_mut().find(in_root) else {
            return false;
        };
        if root.watch.coverage.is_ok() {
            root.watch.coverage = Err(notify::Error::new(ErrorKind::MaxFilesWatch));
            report(&root.path, &root.watch);
            self.fall_back();
        }
        true
    }

    /// Scans the library every `fallback_every` while any root is not wholly watched, and stops
    /// once every root is.
    fn fall_back(&mut self) {
        let Some(every) = self.fallback_every else {
            return;
        };
        let whole = self.roots.iter().all(|root| root.watch.coverage.is_ok());
        if whole == self.fallback.is_none() {
            return;
        }
        let roots: Vec<&Path> = self.roots.iter().map(|root| root.path.as_path()).collect();
        match self.fallback.take() {
            None => {
                warn!(
                    ?roots,
                    ?every,
                    "not every folder is watched, so the library is scanned on a schedule"
                );
                self.fallback = Some(Schedule::start(self.scanner.clone(), every));
            }
            Some(fallback) => {
                info!(?roots, "every folder is watched again, so the fallback schedule stops");
                fallback.stop();
            }
        }
    }

    /// Stops the fallback schedule. The watches end as they are dropped.
    pub(super) fn stop(self) {
        if let Some(fallback) = self.fallback {
            fallback.stop();
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
        Err(error) if limit(error) => {
            warn!(%root, "part of the root is not watched: the OS limit on watches was reached")
        }
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
