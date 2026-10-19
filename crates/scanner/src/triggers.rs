//! Triggers (`requirements/scanning.md` §4, `design/scanning.md` §9).
//!
//! The watcher and the schedule both do one thing: enqueue scopes. The
//! watcher is a hint, never the source of truth — overflow or error enqueues
//! the whole root.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use notify::event::RemoveKind;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::discover::gone;
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
    /// directory over `debounce` and enqueued as one scope each. Folder links
    /// are not watched through, as the walk does not follow them.
    pub fn start(
        scanner: Scanner,
        roots: &[PathBuf],
        debounce: Duration,
    ) -> notify::Result<FsWatcher> {
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = watcher(tx)?;
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
        Ok(FsWatcher { stop, thread: Some(thread), _watcher: watcher })
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A watcher that sends every event to `tx`. Folder links are not watched
/// through, as the walk does not follow them.
fn watcher(tx: mpsc::Sender<notify::Result<notify::Event>>) -> notify::Result<RecommendedWatcher> {
    RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        Config::default().with_follow_symlinks(false),
    )
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
fn classify(
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
        Schedule { stop, thread: Some(thread) }
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
    use crate::store::{MemoryStore, Store};
    use crate::testing::{library, make_album, scan_library, scan_once};
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

    /// The scopes `classify` makes of `events` against what `store` holds under `root`.
    fn scopes(
        store: &MemoryStore,
        root: &Path,
        events: &[(EventKind, Vec<PathBuf>)],
    ) -> Vec<Scope> {
        let mut scopes: Vec<Scope> = events
            .iter()
            .flat_map(|(kind, paths)| {
                paths
                    .iter()
                    .flat_map(|path| classify(kind, path, |p| store.indexes_under(&"1".into(), p)))
            })
            .map(|(dir, depth)| Scope::folder(root, dir, depth))
            .collect();
        scopes.sort_by(|a, b| a.path.cmp(&b.path));
        scopes.dedup();
        scopes
    }

    /// Runs the scopes `classify` makes of `events` as one watcher scan of `root`'s library.
    fn scan_events(store: &MemoryStore, root: &Path, events: &[(EventKind, Vec<PathBuf>)]) {
        let scopes = scopes(store, root, events);
        scan_once(store, &library(root), Trigger::Watch, scopes);
    }

    /// Each track's path, and whether it is missing, sorted by path.
    fn indexed(store: &MemoryStore) -> Vec<(PathBuf, bool)> {
        let mut tracks: Vec<(PathBuf, bool)> =
            store.tracks(&"1".into()).into_iter().map(|t| (t.record.path, t.missing)).collect();
        tracks.sort();
        tracks
    }

    /// A folder renamed, with the events a Linux watcher reports for `mv A B`.
    #[test]
    fn a_renamed_folder_moves_its_tracks() {
        use notify::event::RenameMode;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        make_album(&root, "Artist", "A", 2);
        let store = MemoryStore::new();
        scan_library(&store, &library(&root));
        let (a, b) = (root.join("Artist/A"), root.join("Artist/B"));
        fs::rename(&a, &b).unwrap();

        let name = |mode| EventKind::Modify(ModifyKind::Name(mode));
        scan_events(
            &store,
            &root,
            &[
                (name(RenameMode::From), vec![a.clone()]),
                (name(RenameMode::To), vec![b.clone()]),
                (name(RenameMode::Both), vec![a.clone(), b.clone()]),
            ],
        );
        let track = |dir: &Path, n: u32| dir.join(format!("{n:02} - Track {n}.wav"));
        assert_eq!(
            indexed(&store),
            [
                (track(&a, 1), true),
                (track(&a, 2), true),
                (track(&b, 1), false),
                (track(&b, 2), false)
            ]
        );
    }

    /// A file renamed is a change to its folder only, as nothing is indexed under its old path.
    #[test]
    fn a_renamed_file_dirties_only_its_folder() {
        use notify::event::RenameMode;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        make_album(&root, "Artist", "A", 1);
        let store = MemoryStore::new();
        scan_library(&store, &library(&root));
        let a = root.join("Artist/A");
        let (old, new) = (a.join("01 - Track 1.wav"), a.join("renamed.wav"));
        fs::rename(&old, &new).unwrap();

        let name = |mode| EventKind::Modify(ModifyKind::Name(mode));
        let events =
            [(name(RenameMode::From), vec![old.clone()]), (name(RenameMode::To), vec![new])];
        assert_eq!(scopes(&store, &root, &events), [Scope::folder(&root, &a, Depth::Directory)]);
    }

    /// A file deleted, then a folder deleted, with the events a Linux watcher reports for `rm`
    /// and `rm -r`.
    #[test]
    fn deleted_files_and_folders_go_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        make_album(&root, "Artist", "A", 2);
        make_album(&root, "Artist", "D", 2);
        let store = MemoryStore::new();
        scan_library(&store, &library(&root));
        let (a, d) = (root.join("Artist/A"), root.join("Artist/D"));
        let track = |dir: &Path, n: u32| dir.join(format!("{n:02} - Track {n}.wav"));

        fs::remove_file(track(&a, 1)).unwrap();
        let removed = EventKind::Remove(RemoveKind::File);
        assert_eq!(classify(&removed, &track(&a, 1), unasked), [(a.clone(), Depth::Directory)]);
        scan_events(&store, &root, &[(removed, vec![track(&a, 1)])]);
        assert_eq!(
            indexed(&store),
            [
                (track(&a, 1), true),
                (track(&a, 2), false),
                (track(&d, 1), false),
                (track(&d, 2), false)
            ]
        );

        fs::remove_dir_all(&d).unwrap();
        scan_events(
            &store,
            &root,
            &[
                (EventKind::Remove(RemoveKind::File), vec![track(&d, 1)]),
                (EventKind::Remove(RemoveKind::File), vec![track(&d, 2)]),
                (EventKind::Remove(RemoveKind::Folder), vec![d.clone()]),
            ],
        );
        assert_eq!(
            indexed(&store),
            [
                (track(&a, 1), true),
                (track(&a, 2), false),
                (track(&d, 1), true),
                (track(&d, 2), true)
            ]
        );
        assert_eq!(store.problems(&"1".into()), []);
    }

    #[test]
    fn the_watcher_does_not_follow_a_folder_link() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, outside) = (tmp.path().join("root"), tmp.path().join("outside"));
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        let link = root.join("linked");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut watcher = watcher(tx).unwrap();
        watcher.watch(&root, RecursiveMode::Recursive).unwrap();

        // A watch delivers events in order, so once the sentinel inside the root arrives, any
        // event for the earlier write behind the link would have arrived before it.
        fs::write(outside.join("01.flac"), b"x").unwrap();
        let sentinel = root.join("sentinel");
        fs::write(&sentinel, b"x").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut paths = Vec::new();
        while !paths.contains(&sentinel) {
            let left = deadline.saturating_duration_since(Instant::now());
            paths.extend(rx.recv_timeout(left).expect("the sentinel's event").unwrap().paths);
        }
        assert!(!paths.iter().any(|p| p.starts_with(&link)), "{paths:?}");
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
