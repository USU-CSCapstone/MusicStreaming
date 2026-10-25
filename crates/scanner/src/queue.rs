//! One scanner task per library owning one coalescing queue of scopes
//! (`design/scanning.md` §9).
//!
//! Every trigger does nothing but enqueue scopes. Work that a queued scan
//! not yet started covers joins it, and work that covers queued scans
//! replaces them. A scan that has started, running or resumed from a cursor,
//! takes no new work, since its walk may already have passed that work, so a
//! trigger repeated during a scan queues one follow-up scan, not one each time.
//! The queue writes a new scan's row under its lock, so a busy store holds up the other callers of
//! the queue for one write. A scan suspended by an unavailable root is requested again
//! after a fixed wait ([`Retry`]).

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use crate::library::LibraryConfig;
use crate::scan::{Scan, ScanId, ScanOptions, Trigger};
use crate::scope::{Depth, Scope};
use crate::store::Store;
use jewelcase_core::Recover;
pub use worker::ScannerThread;

mod request;
mod worker;

/// The scopes of a suspended scan whose root did not answer, requested again with its trigger
/// once `due` has passed.
struct Retry {
    due: Instant,
    trigger: Trigger,
    scopes: Vec<Scope>,
}

/// The scans waiting to run, first to run first, the one running, and the retry waiting, if any.
struct Queue {
    queued: VecDeque<Scan>,
    running: Option<Scan>,
    retry: Option<Retry>,
}

impl Queue {
    /// Holds `scopes`, whose root did not answer a scan with `trigger`, to be requested again at
    /// `due`. A retry already waiting takes them in and keeps its time and trigger.
    fn retry_later(&mut self, trigger: Trigger, scopes: Vec<Scope>, due: Instant) {
        match &mut self.retry {
            Some(retry) => {
                retry.scopes.extend(scopes);
                retry.scopes = coalesce_scopes(std::mem::take(&mut retry.scopes));
            }
            None => self.retry = Some(Retry { due, trigger, scopes: coalesce_scopes(scopes) }),
        }
    }

    /// The queued scan, not yet started, that already covers `scopes`, which a request for them
    /// joins.
    fn covering(&self, scopes: &[Scope]) -> Option<ScanId> {
        self.queued.iter().find(|s| s.cursor.is_none() && s.covers(scopes)).map(|s| s.id)
    }
}

/// What a library's [`Scanner`] handles and its thread share.
struct Inner {
    store: Arc<dyn Store>,
    library: LibraryConfig,
    options: ScanOptions,
    queue: Mutex<Queue>,
    /// Notified whenever the queue changes, and once a shutdown is asked for.
    wake: Condvar,
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
    /// work waits for the next trigger.
    pub fn request(&self, trigger: Trigger, scopes: Vec<Scope>) -> Option<ScanId> {
        self.inner.request(trigger, scopes)
    }

    /// Scan every root (`Trigger::Initial`, `Scheduled`, ...). `None` as for
    /// [`Scanner::request`].
    pub fn scan_library(&self, trigger: Trigger) -> Option<ScanId> {
        let roots = self.inner.library.roots.clone();
        self.request(trigger, roots.into_iter().map(Scope::root).collect())
    }

    /// The root containing `path`, if any.
    pub fn root_for(&self, path: &std::path::Path) -> Option<std::path::PathBuf> {
        self.inner.library.roots.iter().find(|r| path.starts_with(r)).cloned()
    }

    /// The library's exclude patterns, as configured.
    pub(crate) fn excludes(&self) -> Vec<String> {
        self.inner.library.excludes.clone()
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
    use std::time::Duration;

    mod retries;
    mod scan_rows;

    /// A second suspension while a retry waits joins it and leaves its time.
    #[test]
    fn a_second_suspension_joins_the_waiting_retry() {
        let (a, b) = (PathBuf::from("/a"), PathBuf::from("/b"));
        let mut queue = Queue { queued: VecDeque::new(), running: None, retry: None };
        let first = Instant::now();
        queue.retry_later(Trigger::Initial, vec![Scope::root(&a)], first);
        queue.retry_later(Trigger::Manual, vec![Scope::root(&b)], first + Duration::from_secs(60));
        let retry = queue.retry.unwrap();
        assert_eq!((retry.due, retry.trigger), (first, Trigger::Initial));
        assert_eq!(retry.scopes, [Scope::root(&a), Scope::root(&b)]);
    }

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
