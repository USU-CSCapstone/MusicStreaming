//! One scanner task per library owning one coalescing queue of scopes
//! (`design/scanning.md` §9).
//!
//! Every trigger does nothing but enqueue scopes. Equal work is dropped,
//! containing work replaces contained work, and the API's "an equivalent
//! scan is already running, return it" is a lookup.

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
    shutdown: bool,
}

struct Inner {
    store: Arc<dyn Store>,
    library: RwLock<LibraryConfig>,
    governor: Arc<Governor>,
    options: ScanOptions,
    queue: Mutex<Queue>,
    wake: Condvar,
    cancel_running: AtomicBool,
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
    /// re-queued with their cursors so nothing is re-walked
    /// (`design/scanning.md` §9).
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
            queue: Mutex::new(Queue {
                queued: VecDeque::new(),
                running: None,
                shutdown: false,
            }),
            wake: Condvar::new(),
            cancel_running: AtomicBool::new(false),
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
        Scanner {
            inner,
            worker: Arc::new(Mutex::new(Some(handle))),
        }
    }

    /// Enqueue work. Returns the id of the scan that will do it, which may be
    /// an existing queued or running scan that already covers it.
    pub fn request(&self, trigger: Trigger, scopes: Vec<Scope>) -> ScanId {
        let scopes = coalesce_scopes(scopes);
        let mut q = self.inner.queue.lock().unwrap();
        if let Some(running) = &q.running
            && running.covers(&scopes)
            && !self.inner.cancel_running.load(Ordering::SeqCst)
        {
            return running.id;
        }
        if let Some(existing) = q.queued.iter().find(|s| s.covers(&scopes)) {
            return existing.id;
        }
        // Replace anything the new work subsumes.
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
        let subsumed: Vec<Scan> = {
            let (keep, drop): (VecDeque<Scan>, VecDeque<Scan>) =
                q.queued.drain(..).partition(|s| !scan.covers(&s.scopes));
            q.queued = keep;
            drop.into_iter().collect()
        };
        for mut s in subsumed {
            s.state = ScanState::Cancelled;
            s.finished_at = Some(std::time::SystemTime::now());
            self.inner.store.update_scan(&s);
        }
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
        self.inner
            .library
            .read()
            .unwrap()
            .roots
            .iter()
            .find(|r| path.starts_with(r))
            .cloned()
    }

    /// Replace the library configuration and reconcile
    /// (`requirements/scanning.md` §7).
    pub fn reconfigure(&self, library: LibraryConfig) -> ScanId {
        *self.inner.library.write().unwrap() = library;
        self.scan_library(Trigger::Reconfigure)
    }

    /// Cancel a queued or running scan. Running scans stop at the next batch
    /// boundary; everything persisted stays persisted.
    pub fn cancel(&self, id: ScanId) -> bool {
        let mut q = self.inner.queue.lock().unwrap();
        if q.running.as_ref().is_some_and(|s| s.id == id) {
            self.inner.cancel_running.store(true, Ordering::SeqCst);
            return true;
        }
        if let Some(pos) = q.queued.iter().position(|s| s.id == id) {
            let mut s = q.queued.remove(pos).unwrap();
            s.state = ScanState::Cancelled;
            s.finished_at = Some(std::time::SystemTime::now());
            self.inner.store.update_scan(&s);
            return true;
        }
        false
    }

    pub fn running(&self) -> Option<Scan> {
        self.inner.queue.lock().unwrap().running.clone()
    }

    pub fn queued(&self) -> Vec<Scan> {
        self.inner
            .queue
            .lock()
            .unwrap()
            .queued
            .iter()
            .cloned()
            .collect()
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

    /// Stop after the current batch and join the worker.
    pub fn shutdown(&self) {
        {
            let mut q = self.inner.queue.lock().unwrap();
            q.shutdown = true;
            self.inner.cancel_running.store(true, Ordering::SeqCst);
        }
        self.inner.wake.notify_all();
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
                if q.shutdown {
                    return;
                }
                if let Some(s) = q.queued.pop_front() {
                    break s;
                }
                q = inner.wake.wait(q).unwrap();
            }
        };
        inner.cancel_running.store(false, Ordering::SeqCst);
        {
            let mut q = inner.queue.lock().unwrap();
            scan.state = ScanState::Running;
            q.running = Some(scan.clone());
        }
        let library = inner.library.read().unwrap().clone();
        let ctx = ScanContext {
            store: inner.store.as_ref(),
            library: &library,
            governor: &inner.governor,
            cancel: &inner.cancel_running,
            options: &inner.options,
        };
        tracing::info!(scan = scan.id, trigger = ?scan.trigger, scopes = scan.scopes.len(), "scan starting");
        run_scan(&ctx, &mut scan);
        tracing::info!(scan = scan.id, state = ?scan.state, progress = ?scan.progress, "scan finished");
        {
            let mut q = inner.queue.lock().unwrap();
            q.running = None;
        }
        inner.wake.notify_all();
    }
}

/// Drop scopes covered by other scopes in the same request.
fn coalesce_scopes(mut scopes: Vec<Scope>) -> Vec<Scope> {
    scopes.dedup();
    let mut out: Vec<Scope> = Vec::new();
    for s in scopes {
        if out.iter().any(|o| o.covers(&s)) {
            continue;
        }
        out.retain(|o| !s.covers(o));
        out.push(s);
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
