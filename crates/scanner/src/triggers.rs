//! Triggers (`requirements/scanning.md` §4, `design/scanning.md` §9).
//!
//! The watcher and the schedule both do one thing: enqueue scopes. The
//! watcher is a hint, never the source of truth — overflow or error enqueues
//! the whole library. `roots` keeps each root watched.

mod roots;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use std::{fs, io};

use globset::GlobSet;
use notify::{Config, EventKind, RecommendedWatcher, Watcher as _};

use crate::discover::compile_excludes;
use crate::queue::Scanner;
use crate::scan::Trigger;
use crate::scope::{Depth, Scope};
use crate::sidecar::is_sidecar_name;
use crate::worker::{Stop, Worker};
use roots::Roots;

/// How often the watcher checks that each root is still the directory it watches.
const CHECK_ROOTS: Duration = Duration::from_secs(10);

/// When the watcher acts.
struct Timing {
    /// How long a folder's changes gather before its scope is queued.
    debounce: Duration,
    /// How often each root is checked for a changed directory.
    check: Duration,
}

/// Watches every root of `scanner`'s library, from a thread of its own, so this returns
/// before the watches are set up. What the library's excludes match as this starts is not
/// watched. Changes are grouped by directory over `debounce` and enqueued as one scope each.
/// Fails if the thread cannot be started. Dropping the worker stops the watcher.
pub fn watch(scanner: Scanner, roots: &[PathBuf], debounce: Duration) -> io::Result<Worker> {
    let timing = Timing { debounce, check: CHECK_ROOTS };
    // Every scan suspends on a pattern that does not compile, so watching the folders it
    // meant to leave out indexes none of them.
    let excludes = compile_excludes(&scanner.excludes()).unwrap_or_else(|error| {
        tracing::warn!(%error, "an exclude pattern does not compile, so every folder is watched");
        GlobSet::empty()
    });
    spawn_watch(scanner, roots.to_vec(), Arc::new(excludes), timing)
}

/// [`watch`] with `excludes` compiled and `timing` given, which tests shorten.
fn spawn_watch(
    scanner: Scanner,
    roots: Vec<PathBuf>,
    excludes: Arc<GlobSet>,
    timing: Timing,
) -> io::Result<Worker> {
    Worker::spawn("scanner-watch", move |stop| {
        let (tx, rx) = mpsc::channel();
        let roots = Roots::start(scanner.clone(), &roots, tx, excludes, &timing, &stop);
        debounce_loop(scanner, roots, rx, &stop, timing.debounce);
    })
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

/// A directory's queued change: how deep its scope goes, and when it was first seen, from which
/// the debounce is counted.
struct Pending {
    depth: Depth,
    first_seen: Instant,
}

/// Queues the scopes `rx`'s events dirty, each once its folder's changes have gathered for
/// `debounce`, and keeps `roots` watched, until `stop` is asked.
fn debounce_loop(
    scanner: Scanner,
    mut roots: Roots,
    rx: mpsc::Receiver<notify::Result<notify::Event>>,
    stop: &Stop,
    debounce: Duration,
) {
    let mut pending: HashMap<PathBuf, Pending> = HashMap::new();
    let mut rescan_all = false;
    while !stop.asked() {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Ok(event)) => {
                if event.need_rescan() {
                    rescan_all = true;
                    roots.watch_all(stop);
                }
                for path in event.paths {
                    roots.watch_shown(&event.kind, &path, stop);
                    for (dir, depth) in classify(&event.kind, &path) {
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
        roots.check(Instant::now(), stop);

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
/// that no longer exists dirties its folder, since a folder removed or renamed away is found
/// missing from its parent's listing. A link to a folder is a change to its parent's listing,
/// never a scope through it.
pub(crate) fn classify(kind: &EventKind, path: &Path) -> Vec<(PathBuf, Depth)> {
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
        assert_eq!(classify(&create, &cover), [(dir.clone(), Depth::Subtree)]);
        let modify = EventKind::Modify(ModifyKind::Any);
        assert_eq!(classify(&modify, &track), [(dir, Depth::Directory)]);
    }

    #[test]
    fn a_folder_link_dirties_only_its_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let (real, link) = (tmp.path().join("real"), tmp.path().join("link"));
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let create = EventKind::Create(CreateKind::Any);
        assert_eq!(classify(&create, &real), [(real.clone(), Depth::Subtree)]);
        assert_eq!(classify(&create, &link), [(tmp.path().to_path_buf(), Depth::Directory)]);
    }
}
