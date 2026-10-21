//! A watcher that fails where a test says and counts its watches, and a wait for the watcher
//! to see a folder.

use std::sync::atomic::{AtomicUsize, Ordering};

use notify::{Config, EventHandler, RecommendedWatcher, RecursiveMode, Watcher, WatcherKind};

use super::*;
use crate::scope::Depth;
use crate::triggers::watcher;

/// A real watcher that fails to watch one folder, with the error `error` makes, and counts
/// every folder it is asked to watch.
pub(super) struct FailsAt {
    watcher: RecommendedWatcher,
    folder: PathBuf,
    error: fn() -> notify::Error,
    calls: Arc<AtomicUsize>,
}

impl FailsAt {
    /// A watch of `root`, sending events to `events`, whose watcher fails at `folder` with the
    /// error `error` makes.
    pub(super) fn watch(
        root: &Path,
        events: &Events,
        folder: PathBuf,
        error: fn() -> notify::Error,
    ) -> RootWatch {
        FailsAt::start(root, events, folder, error, Arc::default())
    }

    /// A watch of `root`, sending events to `events`, that fails nowhere and adds each folder it
    /// is asked to watch to `calls`.
    pub(super) fn counted(root: &Path, events: &Events, calls: &Arc<AtomicUsize>) -> RootWatch {
        let unused = || notify::Error::generic("unused");
        FailsAt::start(root, events, PathBuf::new(), unused, calls.clone())
    }

    /// A watch of `root` with a watcher of these parts.
    fn start(
        root: &Path,
        events: &Events,
        folder: PathBuf,
        error: fn() -> notify::Error,
        calls: Arc<AtomicUsize>,
    ) -> RootWatch {
        let watcher = watcher(events.clone()).unwrap();
        RootWatch::with(Box::new(FailsAt { watcher, folder, error, calls }), root, &Arc::default())
    }
}

impl Watcher for FailsAt {
    /// A watcher that fails nowhere.
    fn new<F: EventHandler>(handler: F, config: Config) -> notify::Result<Self> {
        let watcher = RecommendedWatcher::new(handler, config)?;
        let error = || notify::Error::generic("unused");
        Ok(FailsAt { watcher, folder: PathBuf::new(), error, calls: Arc::default() })
    }

    /// Counts the call, then fails at the folder, and watches anything else.
    fn watch(&mut self, path: &Path, mode: RecursiveMode) -> notify::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if path == self.folder {
            return Err((self.error)().add_path(path.into()));
        }
        self.watcher.watch(path, mode)
    }

    /// Stops watching `path`.
    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        self.watcher.unwatch(path)
    }

    /// The kind of the watcher underneath.
    fn kind() -> WatcherKind {
        RecommendedWatcher::kind()
    }
}

/// The OS limit on watches, as a watch meets it.
pub(super) fn limit_error() -> notify::Error {
    notify::Error::new(ErrorKind::MaxFilesWatch)
}

/// Writes files into `folder`, in `root`, until a watcher scan newer than scan `after` covers
/// just that folder, and returns that scan's id.
pub(super) fn seen(store: &MemoryStore, root: &Path, folder: &Path, after: i64) -> i64 {
    let scope = vec![Scope::folder(root, folder, Depth::Directory)];
    let deadline = Instant::now() + PATIENCE;
    let mut n = 0;
    loop {
        fs::write(folder.join(format!("{n}.txt")), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let scans = scans(store);
        if let Some(scan) = scans.iter().find(|s| s.id > after && s.scopes == scope) {
            break scan.id;
        }
        let folder = folder.display();
        assert!(Instant::now() < deadline, "no watcher scan of {folder} after {after}: {scans:?}");
        n += 1;
    }
}
