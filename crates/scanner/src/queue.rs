//! One scanner task per library owning one coalescing queue of scopes
//! (`design/scanning.md` §9).
//!
//! Every trigger does nothing but enqueue scopes. Work that a queued scan
//! not yet started covers joins it, and work that covers queued scans
//! replaces them. A scan that has started, running or resumed from a cursor,
//! takes no new work, since its walk may already have passed that work, so a
//! trigger repeated during a scan queues one follow-up scan, not one each time.
//! The queue decides under its lock and writes scan rows after, so a busy store
//! holds up only the caller writing. Work a suspended scan left undone is queued
//! again after a wait ([`retry`]).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use crate::library::LibraryConfig;
use crate::lock::Recover;
use crate::scan::{Scan, ScanId, ScanOptions, Trigger};
use crate::scope::{Depth, Scope};
use crate::store::Store;
use retry::Retry;
pub use worker::ScannerThread;

mod request;
#[cfg(test)]
mod retries;
mod retry;
#[cfg(test)]
mod scan_rows;
mod worker;

/// The scans waiting to run, first to run first, the one running, and the work waiting to be
/// retried.
struct Queue {
    queued: VecDeque<Scan>,
    running: Option<Scan>,
    retry: Retry,
}

impl Queue {
    /// The queued scan, not yet started, that already covers `scopes`, which a request for them
    /// joins.
    fn covering(&self, scopes: &[Scope]) -> Option<ScanId> {
        self.queued.iter().find(|s| s.cursor.is_none() && s.covers(scopes)).map(|s| s.id)
    }
}

/// What a library's [`Scanner`] handles and its thread share.
struct Inner {
    store: Arc<dyn Store>,
    library: RwLock<LibraryConfig>,
    options: ScanOptions,
    queue: Mutex<Queue>,
    /// Notified whenever the queue changes, and once a shutdown is asked for.
    wake: Condvar,
    /// Set to cancel the running scan; cleared as each scan starts.
    cancel_running: AtomicBool,
    /// Set once, under the queue lock, so a worker about to wait for work sees it or is woken.
    shutdown: AtomicBool,
}

/// Handle to one library's scanner, for queuing and inspecting its scans. Cloneable; its thread
/// runs until the [`ScannerThread`] that [`Scanner::start`] returns with it is dropped.
#[derive(Clone)]
pub struct Scanner {
    inner: Arc<Inner>,
}

impl Scanner {
    /// Enqueue work. Returns the id of the scan that will do it: a queued scan
    /// that has not started and already covers it, or a new one, never the
    /// running scan or a resumed one. A new scan cancels the queued scans it
    /// covers. `None` when the store cannot record a new scan: the error is logged and the
    /// work is queued again after a wait, as a suspended scan's is.
    pub fn request(&self, trigger: Trigger, scopes: Vec<Scope>) -> Option<ScanId> {
        self.inner.request(trigger, scopes)
    }

    /// Scan every root (`Trigger::Initial`, `Scheduled`, `Reconfigure`, ...). `None` as for
    /// [`Scanner::request`].
    pub fn scan_library(&self, trigger: Trigger) -> Option<ScanId> {
        let roots = self.inner.library.read().recover().roots.clone();
        self.request(trigger, roots.into_iter().map(Scope::root).collect())
    }

    /// The root containing `path`, if any.
    pub fn root_for(&self, path: &std::path::Path) -> Option<std::path::PathBuf> {
        self.inner.library.read().recover().roots.iter().find(|r| path.starts_with(r)).cloned()
    }

    /// The library's exclude patterns, as configured.
    pub(crate) fn excludes(&self) -> Vec<String> {
        self.inner.library.read().recover().excludes.clone()
    }

    /// Whether the index holds a track not yet missing, or a problem, under `folder`, which
    /// may no longer exist. When the store cannot say, the error is logged and the answer is
    /// yes, which costs the watcher only a scan of the folder.
    pub fn indexes_under(&self, folder: &std::path::Path) -> bool {
        let library = self.inner.library.read().recover().id;
        self.inner.store.indexes_under(library, folder).unwrap_or_else(|error| {
            tracing::error!(library, folder = %folder.display(), %error, "index not checked");
            true
        })
    }

    /// Replace the library configuration and reconcile
    /// (`requirements/scanning.md` §7). `None` as for [`Scanner::request`].
    pub fn reconfigure(&self, library: LibraryConfig) -> Option<ScanId> {
        *self.inner.library.write().recover() = library;
        self.scan_library(Trigger::Reconfigure)
    }

    /// Cancel a queued or running scan, returning whether it is cancelled. A running scan stops
    /// after the file in hand and ends cancelled, even if a shutdown follows; everything persisted
    /// stays persisted. A queued scan whose row the store cannot save cancelled goes back in the
    /// queue, and this returns false.
    pub fn cancel(&self, id: ScanId) -> bool {
        let (ahead, scan) = {
            let mut q = self.inner.queue.lock().recover();
            if q.running.as_ref().is_some_and(|s| s.id == id) {
                self.inner.cancel_running.store(true, Ordering::SeqCst);
                return true;
            }
            let Some(position) = q.queued.iter().position(|s| s.id == id) else {
                return false;
            };
            let ahead: Vec<ScanId> = q.queued.iter().take(position).map(|s| s.id).collect();
            let Some(scan) = q.queued.remove(position) else {
                return false;
            };
            (ahead, scan)
        };
        // An idle check waiting on the queue sees it empty now, or still running. Only a stopped
        // worker leaves a scan queued with nothing running for longer than it takes to start it,
        // so this matters only to an idle check after a shutdown.
        self.inner.wake.notify_all();
        if self.inner.cancelled(&scan) {
            return true;
        }
        // Back where it was: after the last scan still queued that was ahead of it, whatever has
        // left the queue meanwhile.
        let mut q = self.inner.queue.lock().recover();
        let position = q.queued.iter().rposition(|s| ahead.contains(&s.id)).map_or(0, |p| p + 1);
        q.queued.insert(position, scan);
        drop(q);
        self.inner.wake.notify_all();
        false
    }

    /// The scan running now, if any.
    #[cfg(feature = "testing")]
    pub fn running(&self) -> Option<Scan> {
        self.inner.queue.lock().recover().running.clone()
    }

    /// The scans waiting to run, first to run first.
    pub fn queued(&self) -> Vec<Scan> {
        self.inner.queue.lock().recover().queued.iter().cloned().collect()
    }

    /// Whether no scan is running or queued.
    #[cfg(feature = "testing")]
    pub fn is_idle(&self) -> bool {
        let q = self.inner.queue.lock().recover();
        q.running.is_none() && q.queued.is_empty()
    }

    /// Block until the queue is empty and nothing is running.
    #[cfg(feature = "testing")]
    pub fn wait_idle(&self) {
        let mut q = self.inner.queue.lock().recover();
        while q.running.is_some() || !q.queued.is_empty() {
            q = self.inner.wake.wait(q).recover();
        }
    }

    /// Shortens the wait of the work waiting to be retried to `wait` from now, after which the
    /// worker queues it, and returns whether any waits.
    #[cfg(feature = "testing")]
    pub fn retry_in(&self, wait: std::time::Duration) -> bool {
        let waits = self.inner.queue.lock().recover().retry.hurry(std::time::Instant::now() + wait);
        self.inner.wake.notify_all();
        waits
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
