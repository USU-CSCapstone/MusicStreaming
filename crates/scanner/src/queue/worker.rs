//! The library's scanner thread, which runs its scans one at a time, and the one owner that stops
//! it (`design/scanning.md` §9).

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use super::{Inner, Queue, Scanner};
use crate::library::LibraryConfig;
use crate::scan::{ScanContext, ScanOptions, ScanState, run_scan};
use crate::store::Store;
use crate::worker::Worker;
use jewelcase_core::Recover;

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

    /// [`Scanner::start`], with scans batching and retrying as `options` say.
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
            library,
            options,
            queue: Mutex::new(Queue { queued: VecDeque::new(), running: None, retry: None }),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        {
            let mut q = inner.queue.lock().recover();
            for mut scan in interrupted {
                scan.state = ScanState::Queued;
                q.queued.push_back(scan);
            }
        }
        let running = inner.clone();
        // The thread waits on the queue's condvar, not on its `Stop`, since requests already wake
        // it there and its wait ends when a retry is due, and a running scan checks the `shutdown` flag before each
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
        let queue = self.inner.queue.lock().recover();
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

/// Runs the library's scans one at a time, and requests the scopes of the scan waiting to be
/// retried once its wait has passed, until a shutdown.
fn run(inner: &Inner) {
    loop {
        let mut scan = {
            let mut q = inner.queue.lock().recover();
            loop {
                if inner.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(retry) = q.retry.take_if(|r| r.due <= Instant::now()) {
                    drop(q);
                    inner.request(retry.trigger, retry.scopes);
                    q = inner.queue.lock().recover();
                    continue;
                }
                if let Some(mut s) = q.queued.pop_front() {
                    // Running from the moment it leaves the queue, under the same lock, so a
                    // request or idle check always finds it in one place or the other.
                    s.state = ScanState::Running;
                    q.running = Some(s.clone());
                    break s;
                }
                q = match q.retry.as_ref().map(|r| r.due) {
                    Some(due) => {
                        let wait = due.saturating_duration_since(Instant::now());
                        inner.wake.wait_timeout(q, wait).recover().0
                    }
                    None => inner.wake.wait(q).recover(),
                };
            }
        };
        let ctx = ScanContext {
            store: inner.store.as_ref(),
            library: &inner.library,
            shutdown: &inner.shutdown,
            options: &inner.options,
        };
        tracing::info!(scan = scan.id, trigger = ?scan.trigger, scopes = scan.scopes.len(), "scan starting");
        let unanswered = run_scan(&ctx, &mut scan);
        if scan.state == ScanState::Running {
            tracing::info!(scan = scan.id, progress = ?scan.progress, "scan stopped for shutdown");
        } else {
            tracing::info!(scan = scan.id, state = ?scan.state, progress = ?scan.progress, "scan finished");
        }
        {
            let mut q = inner.queue.lock().recover();
            q.running = None;
            if !unanswered.is_empty() {
                tracing::info!(scan = scan.id, "a root did not answer; retrying after a wait");
                let due = Instant::now() + inner.options.retry_wait;
                q.retry_later(scan.trigger, unanswered, due);
            }
        }
        inner.wake.notify_all();
    }
}
