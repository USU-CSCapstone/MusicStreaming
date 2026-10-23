//! The library's scanner thread, which runs its scans one at a time, and the one owner that stops
//! it (`design/scanning.md` §9).

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::Instant;

use super::retry::Retry;
use super::{Inner, Queue, Scanner};
use crate::library::LibraryConfig;
use crate::scan::{ScanContext, ScanOptions, ScanState, run_scan};
use crate::store::Store;
use crate::worker::Worker;

/// The one owner of a library's scanner thread. Dropping it stops the thread as
/// [`ScannerThread::stop`] does and waits for it to finish.
#[must_use = "dropping it stops the scanner"]
pub struct ScannerThread {
    inner: Arc<Inner>,
    thread: Worker,
}

impl Scanner {
    /// Starts the library's scanner thread, returning a handle to the scanner and the owner of
    /// its thread. Interrupted scans from a previous process are re-queued with their cursors,
    /// so they continue where they stopped instead of walking from the start. Folders that could
    /// not be read before the cursor are not remembered (`design/scanning.md` §9). If the store
    /// cannot list them, none is resumed now; their rows stay as they are for the next start.
    /// Fails if the thread cannot be started.
    pub fn start(
        store: Arc<dyn Store>,
        library: LibraryConfig,
    ) -> io::Result<(Scanner, ScannerThread)> {
        Scanner::start_with(store, library, ScanOptions::default())
    }

    /// [`Scanner::start`], with scans batching as `options` say.
    pub(crate) fn start_with(
        store: Arc<dyn Store>,
        library: LibraryConfig,
        options: ScanOptions,
    ) -> io::Result<(Scanner, ScannerThread)> {
        let interrupted = store.interrupted_scans(library.id).unwrap_or_else(|error| {
            tracing::error!(library = library.id, %error, "interrupted scans not resumed");
            Vec::new()
        });
        let inner = Arc::new(Inner {
            store,
            library: RwLock::new(library),
            options,
            queue: Mutex::new(Queue {
                queued: VecDeque::new(),
                running: None,
                retry: Retry::new(),
            }),
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
        let running = inner.clone();
        // The thread waits on the queue's condvar, not on its `Stop`, since requests, cancels and
        // retries already wake it there, and a running scan checks the `shutdown` flag before each
        // file; `ScannerThread::stop` sets that flag.
        let thread = Worker::spawn("scanner", move |_| run(&running))?;
        Ok((Scanner { inner: inner.clone() }, ScannerThread { inner, thread }))
    }
}

impl ScannerThread {
    /// Asks the thread to stop without waiting for it, so that several libraries' threads can
    /// be asked before any is waited for; dropping this then waits. The running scan stops after
    /// the file in hand and stays running at its last flushed cursor, and queued scans stay
    /// queued, so the next [`Scanner::start`] resumes them as it would after a crash.
    pub fn stop(&self) {
        let queue = self.inner.queue.lock().unwrap();
        self.inner.shutdown.store(true, Ordering::SeqCst);
        drop(queue);
        self.inner.wake.notify_all();
    }
}

impl Drop for ScannerThread {
    /// [`ScannerThread::stop`], then waits for the thread as [`Worker::stop`] does.
    fn drop(&mut self) {
        self.stop();
        self.thread.stop();
    }
}

/// Runs the library's scans one at a time, and queues the work waiting to be retried once it is
/// due, until a shutdown.
fn run(inner: &Inner) {
    loop {
        let mut scan = {
            let mut q = inner.queue.lock().unwrap();
            loop {
                if inner.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if let Some((trigger, mut scopes)) = q.retry.take_due(Instant::now()) {
                    drop(q);
                    // A reconfiguration may have removed a root since the work was left undone.
                    let library = inner.library.read().unwrap();
                    scopes.retain(|scope| library.roots.contains(&scope.root));
                    drop(library);
                    if scopes.is_empty() {
                        tracing::debug!(
                            ?trigger,
                            "work left undone not retried; its roots have left the library"
                        );
                    } else {
                        inner.request(trigger, scopes);
                    }
                    q = inner.queue.lock().unwrap();
                    continue;
                }
                if let Some(mut s) = q.queued.pop_front() {
                    // Running from the moment it leaves the queue, under the same lock, so a
                    // request, cancel, or idle check always finds it in one place or the other.
                    s.state = ScanState::Running;
                    inner.cancel_running.store(false, Ordering::SeqCst);
                    q.retry.started(&s);
                    q.running = Some(s.clone());
                    break s;
                }
                q = match q.retry.due() {
                    Some(due) => {
                        let wait = due.saturating_duration_since(Instant::now());
                        inner.wake.wait_timeout(q, wait).unwrap().0
                    }
                    None => inner.wake.wait(q).unwrap(),
                };
            }
        };
        let library = inner.library.read().unwrap().clone();
        let ctx = ScanContext {
            store: inner.store.as_ref(),
            library: &library,
            cancel: &inner.cancel_running,
            shutdown: &inner.shutdown,
            options: &inner.options,
        };
        tracing::info!(scan = scan.id, trigger = ?scan.trigger, scopes = scan.scopes.len(), "scan starting");
        let undone = run_scan(&ctx, &mut scan);
        if scan.state == ScanState::Running {
            tracing::info!(scan = scan.id, progress = ?scan.progress, "scan stopped for shutdown");
        } else {
            tracing::info!(scan = scan.id, state = ?scan.state, progress = ?scan.progress, "scan finished");
        }
        {
            let mut q = inner.queue.lock().unwrap();
            q.running = None;
            q.retry.ended(&scan, undone, Instant::now());
        }
        inner.wake.notify_all();
    }
}
