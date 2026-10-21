//! Triggers (`requirements/scanning.md` §4, `design/scanning.md` §9).
//!
//! The watcher and the schedule both do one thing: enqueue scopes. The
//! watcher is a hint, never the source of truth — overflow or error enqueues
//! the whole library. `roots` keeps each root watched.

mod roots;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use std::{fs, io};

use globset::GlobSet;
use notify::event::RemoveKind;
use notify::{Config, EventKind, RecommendedWatcher, Watcher as _};

use crate::discover::{compile_excludes, gone};
use crate::queue::Scanner;
use crate::scan::Trigger;
use crate::scope::{Depth, Scope};
use crate::sidecar::is_sidecar_name;
use crate::worker::Worker;
use roots::{Events, RootWatch, Roots};

/// How often the watcher checks that each root is still the directory it watches.
const CHECK_ROOTS: Duration = Duration::from_secs(10);

/// How long dropping an [`FsWatcher`] waits for the watcher's thread to finish.
const STOP_WAIT: Duration = Duration::from_secs(1);

/// Live filesystem watching with per-directory debouncing, until it is dropped.
#[must_use = "dropping it stops the watcher"]
pub struct FsWatcher {
    stop: Arc<AtomicBool>,
    /// Disconnected once the watcher's thread has finished.
    finished: mpsc::Receiver<()>,
}

/// When the watcher acts.
struct Timing {
    /// How long a folder's changes gather before its scope is queued.
    debounce: Duration,
    /// How often each root is checked for a lost watch.
    check: Duration,
    /// How often the library is scanned while part of it is not watched; `None` when it has a
    /// schedule of its own.
    fallback: Option<Duration>,
}

impl FsWatcher {
    /// Watches every root of `scanner`'s library, from a thread of its own, so this returns
    /// before the watches are set up. What the library's excludes match as this starts is not
    /// watched. Changes are grouped by directory over `debounce` and enqueued as one scope each.
    /// While part of the library is not watched, it is scanned every `fallback`, when given.
    /// Fails if the thread cannot be started.
    pub fn start(
        scanner: Scanner,
        roots: &[PathBuf],
        debounce: Duration,
        fallback: Option<Duration>,
    ) -> io::Result<FsWatcher> {
        let timing = Timing { debounce, check: CHECK_ROOTS, fallback };
        // Every scan suspends on a pattern that does not compile, so watching the folders it
        // meant to leave out indexes none of them.
        let excludes = compile_excludes(&scanner.excludes()).unwrap_or_else(|error| {
            tracing::warn!(%error, "an exclude pattern does not compile, so every folder is watched");
            GlobSet::empty()
        });
        let excludes = Arc::new(excludes);
        let watch = move |root: &Path, events: &Events| RootWatch::start(root, &excludes, events);
        FsWatcher::spawn(scanner, roots.to_vec(), watch, timing)
    }

    /// [`FsWatcher::start`], setting each root's watch up with `watch`, which tests replace to
    /// control what the watch finds.
    fn spawn<W>(
        scanner: Scanner,
        roots: Vec<PathBuf>,
        watch: W,
        timing: Timing,
    ) -> io::Result<FsWatcher>
    where
        W: Fn(&Path, &Events) -> RootWatch + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let (done, finished) = mpsc::channel();
        std::thread::Builder::new().name("scanner-watch".into()).spawn(move || {
            let (tx, rx) = mpsc::channel();
            let roots = Roots::start(scanner.clone(), &roots, tx, watch, &timing);
            debounce_loop(scanner, roots, rx, stop2, timing.debounce);
            drop(done);
        })?;
        Ok(FsWatcher { stop, finished })
    }
}

impl Drop for FsWatcher {
    /// Stops the watcher, waiting up to [`STOP_WAIT`] for its thread, so the scanner it holds
    /// is let go before the process ends. A thread still setting a watch up, or held by a root
    /// that does not answer, as a hard-mounted share that has gone away, is left to finish
    /// alone: all it can still do is queue a scan, which a stopped scanner keeps for its next
    /// start.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Err(RecvTimeoutError::Timeout) = self.finished.recv_timeout(STOP_WAIT) {
            tracing::warn!("the watcher is busy, so it is left to stop on its own");
        }
    }
}

/// A watcher that sends every event to `tx`, until the watcher's thread has finished with them.
fn watcher(tx: mpsc::Sender<notify::Result<notify::Event>>) -> notify::Result<RecommendedWatcher> {
    RecommendedWatcher::new(
        move |res| {
            if tx.send(res).is_err() {
                tracing::debug!("the watcher has stopped, so an event is dropped");
            }
        },
        Config::default(),
    )
}

/// Pending work per directory: whether anything in it needs the subtree.
struct Pending {
    depth: Depth,
    first_seen: Instant,
}

/// Queues the scopes `rx`'s events dirty, each once its folder's changes have gathered for
/// `debounce`, and keeps `roots` watched, until `stop` is set.
fn debounce_loop<W: Fn(&Path, &Events) -> RootWatch>(
    scanner: Scanner,
    mut roots: Roots<W>,
    rx: mpsc::Receiver<notify::Result<notify::Event>>,
    stop: Arc<AtomicBool>,
    debounce: Duration,
) {
    let mut pending: HashMap<PathBuf, Pending> = HashMap::new();
    let mut rescan_all = false;
    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Ok(event)) => {
                if event.need_rescan() {
                    rescan_all = true;
                    roots.watch_all();
                }
                for path in event.paths {
                    roots.watch_shown(&event.kind, &path);
                    let indexed = |path: &Path| scanner.indexes_under(path);
                    for (dir, depth) in classify(&event.kind, &path, indexed) {
                        pending
                            .entry(dir)
                            .and_modify(|p| {
                                if depth == Depth::Subtree {
                                    p.depth = Depth::Subtree;
                                }
                            })
                            .or_insert(Pending { depth, first_seen: Instant::now() });
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "watcher error; rescanning library");
                rescan_all = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        roots.check(Instant::now());

        if rescan_all {
            pending.clear();
            scanner.scan_library(Trigger::Watch);
            rescan_all = false;
            continue;
        }

        let now = Instant::now();
        let ready: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, p)| now.duration_since(p.first_seen) >= debounce)
            .map(|(d, _)| d.clone())
            .collect();
        if ready.is_empty() {
            continue;
        }
        let mut scopes = Vec::new();
        for dir in ready {
            let Some(p) = pending.remove(&dir) else {
                continue;
            };
            if let Some(root) = scanner.root_for(&dir) {
                scopes.push(Scope::folder(root, dir, p.depth));
            }
        }
        if !scopes.is_empty() {
            scanner.request(Trigger::Watch, scopes);
        }
    }
}

/// Which scopes a changed path dirties, each a folder and how deep. A changed file dirties its
/// folder, or the folder's subtree for a sidecar-named file, since anything below may have
/// inherited from it. A folder dirties its own subtree, since its contents are unknown. A path
/// that no longer exists dirties its folder, and its own subtree too when it was a folder: one
/// the event says was removed, or, for any other event such as a rename, one under which
/// `indexed` says the index holds something. A link to a folder is a change to its parent's
/// listing, never a scope through it.
pub(crate) fn classify(
    kind: &EventKind,
    path: &Path,
    indexed: impl FnOnce(&Path) -> bool,
) -> Vec<(PathBuf, Depth)> {
    if matches!(kind, EventKind::Access(_)) {
        return Vec::new();
    }
    let (Some(name), Some(parent)) = (path.file_name(), path.parent()) else {
        return Vec::new();
    };
    let depth = if is_sidecar_name(name) { Depth::Subtree } else { Depth::Directory };
    let folder = (parent.to_path_buf(), depth);
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => vec![(path.to_path_buf(), Depth::Subtree)],
        Err(error) if gone(&error) => {
            let was_folder = match kind {
                EventKind::Remove(RemoveKind::File) => false,
                EventKind::Remove(RemoveKind::Folder) => true,
                _ => indexed(path),
            };
            if was_folder {
                vec![(path.to_path_buf(), Depth::Subtree), folder]
            } else {
                vec![folder]
            }
        }
        _ => vec![folder],
    }
}

/// The schedule, periodic reconciliation that is the safety net for network mounts: scans
/// `scanner`'s library every `interval`, until the worker is dropped.
pub fn scan_every(scanner: Scanner, interval: Duration) -> io::Result<Worker> {
    Worker::spawn("scanner-schedule", move |stop| {
        while !stop.wait(interval) {
            scanner.scan_library(Trigger::Scheduled);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind};

    #[test]
    fn sidecars_widen_to_subtree() {
        let tmp = tempfile::tempdir().unwrap();
        let (cover, track) = (tmp.path().join("cover.jpg"), tmp.path().join("01.flac"));
        fs::write(&cover, b"x").unwrap();
        fs::write(&track, b"x").unwrap();
        let dir = tmp.path().to_path_buf();
        let create = EventKind::Create(CreateKind::File);
        assert_eq!(classify(&create, &cover, unasked), [(dir.clone(), Depth::Subtree)]);
        let modify = EventKind::Modify(ModifyKind::Any);
        assert_eq!(classify(&modify, &track, unasked), [(dir, Depth::Directory)]);
    }

    /// The `indexed` argument for a `classify` call on a path that exists, which must never ask
    /// what the index holds: it panics.
    fn unasked(path: &Path) -> bool {
        panic!("asked what is indexed under {}", path.display())
    }

    #[test]
    fn a_folder_link_dirties_only_its_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let (real, link) = (tmp.path().join("real"), tmp.path().join("link"));
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let create = EventKind::Create(CreateKind::Any);
        assert_eq!(classify(&create, &real, unasked), [(real.clone(), Depth::Subtree)]);
        assert_eq!(
            classify(&create, &link, unasked),
            [(tmp.path().to_path_buf(), Depth::Directory)]
        );
    }
}
