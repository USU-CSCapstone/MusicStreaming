//! One scanner task per library owning one coalescing queue of scopes
//! (`design/scanning.md` §9).
//!
//! Every trigger does nothing but enqueue scopes. Work that a queued scan
//! not yet started covers joins it, and work that covers queued scans
//! replaces them. A scan that has started, running or resumed from a cursor,
//! takes no new work, since its walk may already have passed that work, so a
//! trigger repeated during a scan queues one follow-up scan, not one each time.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::JoinHandle;

use crate::governor::Governor;
use crate::scan::{ScanContext, ScanOptions, run_scan};
use crate::store::Store;
use crate::types::*;

struct Queue {
    queued: VecDeque<Scan>,
    running: Option<Scan>,
}

struct Inner {
    store: Arc<dyn Store>,
    library: RwLock<LibraryConfig>,
    governor: Arc<Governor>,
    options: ScanOptions,
    queue: Mutex<Queue>,
    wake: Condvar,
    cancel_running: AtomicBool,
    /// Set once, under the queue lock, so a worker about to wait for work sees it or is woken.
    shutdown: AtomicBool,
}

/// Handle to one library's scanner. Cloneable; the worker thread lives as
/// long as any handle does and is joined by [`Scanner::shutdown`].
#[derive(Clone)]
pub struct Scanner {
    inner: Arc<Inner>,
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl Scanner {
    /// Start the worker. Interrupted scans from a previous process are
    /// re-queued with their cursors, so they continue where they stopped
    /// instead of walking from the start. Folders that could not be read before the
    /// cursor are not remembered (`design/scanning.md` §9).
    pub fn start(
        store: Arc<dyn Store>,
        library: LibraryConfig,
        governor: Arc<Governor>,
        options: ScanOptions,
    ) -> Scanner {
        let interrupted = store.interrupted_scans(&library.id);
        let inner = Arc::new(Inner {
            store,
            library: RwLock::new(library),
            governor,
            options,
            queue: Mutex::new(Queue { queued: VecDeque::new(), running: None }),
            wake: Condvar::new(),
            cancel_running: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        });
        {
            let mut q = inner.queue.lock().unwrap();
            for mut scan in interrupted {
                scan.state = ScanState::Queued;
                q.queued.push_back(scan);
            }
        }
        let worker_inner = inner.clone();
        let handle = std::thread::Builder::new()
            .name("scanner".into())
            .spawn(move || worker(worker_inner))
            .expect("spawn scanner worker");
        Scanner { inner, worker: Arc::new(Mutex::new(Some(handle))) }
    }

    /// Enqueue work. Returns the id of the scan that will do it: a queued scan
    /// that has not started and already covers it, or a new one, never the
    /// running scan or a resumed one. A new scan cancels the queued scans it
    /// covers.
    pub fn request(&self, trigger: Trigger, scopes: Vec<Scope>) -> ScanId {
        let scopes = coalesce_scopes(scopes);
        let mut q = self.inner.queue.lock().unwrap();
        if let Some(existing) = q.queued.iter().find(|s| s.cursor.is_none() && s.covers(&scopes)) {
            return existing.id;
        }
        let mut scan = Scan {
            id: 0,
            library: self.inner.library.read().unwrap().id.clone(),
            trigger,
            scopes,
            state: ScanState::Queued,
            started_at: None,
            finished_at: None,
            progress: ScanProgress::default(),
            cursor: None,
        };
        q.queued.retain_mut(|queued| {
            let subsumed = scan.covers(&queued.scopes);
            if subsumed {
                self.cancelled(queued);
            }
            !subsumed
        });
        scan.id = self.inner.store.create_scan(&scan);
        let id = scan.id;
        q.queued.push_back(scan);
        drop(q);
        self.inner.wake.notify_all();
        id
    }

    /// Scan every root (`Trigger::Initial`, `Scheduled`, `Reconfigure`, ...).
    pub fn scan_library(&self, trigger: Trigger) -> ScanId {
        let roots = self.inner.library.read().unwrap().roots.clone();
        self.request(trigger, roots.into_iter().map(Scope::root).collect())
    }

    /// Scan one folder inside a root, recursively.
    pub fn scan_folder(&self, trigger: Trigger, path: &std::path::Path) -> Option<ScanId> {
        let root = self.root_for(path)?;
        Some(self.request(trigger, vec![Scope::folder(root, path, Depth::Subtree)]))
    }

    /// The root containing `path`, if any.
    pub fn root_for(&self, path: &std::path::Path) -> Option<std::path::PathBuf> {
        self.inner.library.read().unwrap().roots.iter().find(|r| path.starts_with(r)).cloned()
    }

    /// Whether the index holds a track not yet missing, or a problem, under `folder`, which
    /// may no longer exist.
    pub fn indexes_under(&self, folder: &std::path::Path) -> bool {
        let library = self.inner.library.read().unwrap().id.clone();
        self.inner.store.indexes_under(&library, folder)
    }

    /// Replace the library configuration and reconcile
    /// (`requirements/scanning.md` §7).
    pub fn reconfigure(&self, library: LibraryConfig) -> ScanId {
        *self.inner.library.write().unwrap() = library;
        self.scan_library(Trigger::Reconfigure)
    }

    /// Cancel a queued or running scan. A running scan stops after the file in hand and ends
    /// cancelled, even if a shutdown follows; everything persisted stays persisted.
    pub fn cancel(&self, id: ScanId) -> bool {
        let mut q = self.inner.queue.lock().unwrap();
        if q.running.as_ref().is_some_and(|s| s.id == id) {
            self.inner.cancel_running.store(true, Ordering::SeqCst);
            return true;
        }
        if let Some(mut s) =
            q.queued.iter().position(|s| s.id == id).and_then(|p| q.queued.remove(p))
        {
            self.cancelled(&mut s);
            return true;
        }
        false
    }

    /// Marks `scan`, already taken off the queue, cancelled and saves it to the store.
    fn cancelled(&self, scan: &mut Scan) {
        scan.state = ScanState::Cancelled;
        scan.finished_at = Some(std::time::SystemTime::now());
        self.inner.store.update_scan(scan);
    }

    pub fn running(&self) -> Option<Scan> {
        self.inner.queue.lock().unwrap().running.clone()
    }

    pub fn queued(&self) -> Vec<Scan> {
        self.inner.queue.lock().unwrap().queued.iter().cloned().collect()
    }

    pub fn is_idle(&self) -> bool {
        let q = self.inner.queue.lock().unwrap();
        q.running.is_none() && q.queued.is_empty()
    }

    /// Block until the queue is empty and nothing is running.
    pub fn wait_idle(&self) {
        let mut q = self.inner.queue.lock().unwrap();
        while q.running.is_some() || !q.queued.is_empty() {
            q = self.inner.wake.wait(q).unwrap();
        }
    }

    /// Ask the worker to stop without waiting for it. The running scan stops after the file in
    /// hand and stays running at its last flushed cursor, and queued scans stay queued, so the
    /// next [`Scanner::start`] resumes them as it would after a crash.
    pub(crate) fn stop(&self) {
        let queue = self.inner.queue.lock().unwrap();
        self.inner.shutdown.store(true, Ordering::SeqCst);
        drop(queue);
        self.inner.wake.notify_all();
    }

    /// [`Scanner::stop`], then wait for the worker to finish.
    pub fn shutdown(&self) {
        self.stop();
        if let Some(h) = self.worker.lock().unwrap().take() {
            let _ = h.join();
        }
    }
}

fn worker(inner: Arc<Inner>) {
    loop {
        let mut scan = {
            let mut q = inner.queue.lock().unwrap();
            loop {
                if inner.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(mut s) = q.queued.pop_front() {
                    // Running from the moment it leaves the queue, under the same lock, so a
                    // request, cancel, or idle check always finds it in one place or the other.
                    s.state = ScanState::Running;
                    inner.cancel_running.store(false, Ordering::SeqCst);
                    q.running = Some(s.clone());
                    break s;
                }
                q = inner.wake.wait(q).unwrap();
            }
        };
        let library = inner.library.read().unwrap().clone();
        let ctx = ScanContext {
            store: inner.store.as_ref(),
            library: &library,
            governor: &inner.governor,
            cancel: &inner.cancel_running,
            shutdown: &inner.shutdown,
            options: &inner.options,
        };
        tracing::info!(scan = scan.id, trigger = ?scan.trigger, scopes = scan.scopes.len(), "scan starting");
        run_scan(&ctx, &mut scan);
        if scan.state == ScanState::Running {
            tracing::info!(scan = scan.id, progress = ?scan.progress, "scan stopped for shutdown");
        } else {
            tracing::info!(scan = scan.id, state = ?scan.state, progress = ?scan.progress, "scan finished");
        }
        {
            let mut q = inner.queue.lock().unwrap();
            q.running = None;
        }
        inner.wake.notify_all();
    }
}

/// Drop scopes covered by other scopes in the same request, returning the rest sorted by root
/// and path.
fn coalesce_scopes(mut scopes: Vec<Scope>) -> Vec<Scope> {
    // Paths order by component, so everything a Subtree scope covers sorts directly after it,
    // with a Subtree scope placed before a Directory scope at the same path. Whatever covers a
    // scope is therefore the last one kept before it.
    scopes.sort_by(|a, b| {
        (&a.root, &a.path, a.depth != Depth::Subtree).cmp(&(
            &b.root,
            &b.path,
            b.depth != Depth::Subtree,
        ))
    });
    let mut out: Vec<Scope> = Vec::new();
    for s in scopes {
        if !out.last().is_some_and(|last| last.covers(&s)) {
            out.push(s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn scopes_coalesce() {
        let root = PathBuf::from("/m");
        let a = Scope::folder(&root, "/m/a", Depth::Subtree);
        let ab = Scope::folder(&root, "/m/a/b", Depth::Directory);
        let c = Scope::folder(&root, "/m/c", Depth::Directory);
        let all = Scope::root(&root);
        assert_eq!(
            coalesce_scopes(vec![ab.clone(), a.clone(), c.clone()]),
            vec![a.clone(), c.clone()]
        );
        assert_eq!(coalesce_scopes(vec![a, c, all.clone()]), vec![all]);
    }

    /// Sorting keeps a Subtree scope next to all it covers: a sibling such as `a b`, which sorts
    /// between `a` and `a/b` as text, a Directory scope at the same path, and another root.
    #[test]
    fn scopes_coalesce_in_any_order() {
        let (m, n) = (PathBuf::from("/m"), PathBuf::from("/n"));
        let scope = |root: &PathBuf, path: &str, depth| Scope::folder(root, path, depth);
        let a = scope(&m, "/m/a", Depth::Subtree);
        let a_dir = scope(&m, "/m/a", Depth::Directory);
        let a_b = scope(&m, "/m/a/b", Depth::Subtree);
        let sibling = scope(&m, "/m/a b", Depth::Directory);
        let other_root = scope(&n, "/n/a/b", Depth::Directory);
        let c = scope(&m, "/m/c", Depth::Directory);
        assert_eq!(
            coalesce_scopes(vec![
                c.clone(),
                a_b.clone(),
                sibling.clone(),
                other_root.clone(),
                a_dir.clone(),
                a.clone(),
                c.clone(),
                a_b
            ]),
            [a, sibling, c, other_root]
        );
    }

    #[test]
    fn directory_depth_covers_only_itself() {
        let root = PathBuf::from("/m");
        let dir = Scope::folder(&root, "/m/a", Depth::Directory);
        let sub = Scope::folder(&root, "/m/a", Depth::Subtree);
        assert!(!dir.covers(&sub));
        assert!(sub.covers(&dir));
        assert!(dir.covers(&dir.clone()));
    }
}
