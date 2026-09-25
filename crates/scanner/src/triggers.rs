//! Triggers (`requirements/scanning.md` §4, `design/scanning.md` §9).
//!
//! The watcher and the schedule both do one thing: enqueue scopes. The
//! watcher is a hint, never the source of truth — overflow or error enqueues
//! the whole root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::queue::Scanner;
use crate::sidecar::is_sidecar_name;
use crate::types::{Depth, Scope, Trigger};

/// Live filesystem watching with per-directory debouncing.
pub struct FsWatcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    // Dropped with the struct, which stops the OS watch.
    _watcher: RecommendedWatcher,
}

impl FsWatcher {
    /// Watch every root of `scanner`'s library. Changes are grouped by
    /// directory over `debounce` and enqueued as one scope each.
    pub fn start(
        scanner: Scanner,
        roots: &[PathBuf],
        debounce: Duration,
    ) -> notify::Result<FsWatcher> {
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;
        for root in roots {
            if let Err(e) = watcher.watch(root, RecursiveMode::Recursive) {
                tracing::warn!(root = %root.display(), error = %e, "could not watch root; scheduled scans cover it");
            }
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("scanner-watch".into())
            .spawn(move || debounce_loop(scanner, rx, stop2, debounce))
            .expect("spawn watcher thread");
        Ok(FsWatcher {
            stop,
            thread: Some(thread),
            _watcher: watcher,
        })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Pending work per directory: whether anything in it needs the subtree.
struct Pending {
    depth: Depth,
    first_seen: Instant,
}

fn debounce_loop(
    scanner: Scanner,
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
                }
                for path in event.paths {
                    let Some((dir, depth)) = classify(&event.kind, &path) else {
                        continue;
                    };
                    pending
                        .entry(dir)
                        .and_modify(|p| {
                            if depth == Depth::Subtree {
                                p.depth = Depth::Subtree;
                            }
                        })
                        .or_insert(Pending {
                            depth,
                            first_seen: Instant::now(),
                        });
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "watcher error; rescanning library");
                rescan_all = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

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

/// Which directory a changed path dirties, and how deep. A sidecar-named
/// file widens to the subtree, since anything below may have inherited from
/// it; a directory event does too, since its contents are unknown.
fn classify(kind: &EventKind, path: &Path) -> Option<(PathBuf, Depth)> {
    if matches!(kind, EventKind::Access(_)) {
        return None;
    }
    let name = path.file_name()?;
    let is_dir_event =
        path.is_dir() || matches!(kind, EventKind::Remove(notify::event::RemoveKind::Folder));
    if is_dir_event {
        // A directory created, removed, or renamed: its parent's listing and
        // its own subtree changed.
        return Some((path.to_path_buf(), Depth::Subtree));
    }
    let dir = path.parent()?.to_path_buf();
    if is_sidecar_name(name) {
        return Some((dir, Depth::Subtree));
    }
    Some((dir, Depth::Directory))
}

/// Periodic reconciliation: the safety net for network mounts.
pub struct Schedule {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Schedule {
    pub fn start(scanner: Scanner, interval: Duration) -> Schedule {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("scanner-schedule".into())
            .spawn(move || {
                let mut next = Instant::now() + interval;
                while !stop2.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(250));
                    if Instant::now() >= next {
                        scanner.scan_library(Trigger::Scheduled);
                        next = Instant::now() + interval;
                    }
                }
            })
            .expect("spawn schedule thread");
        Schedule {
            stop,
            thread: Some(thread),
        }
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind};

    #[test]
    fn sidecars_widen_to_subtree() {
        let (dir, depth) = classify(
            &EventKind::Create(CreateKind::File),
            Path::new("/m/a/cover.jpg"),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/m/a"));
        assert_eq!(depth, Depth::Subtree);
        let (dir, depth) = classify(
            &EventKind::Modify(ModifyKind::Any),
            Path::new("/m/a/01.flac"),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/m/a"));
        assert_eq!(depth, Depth::Directory);
    }
}
