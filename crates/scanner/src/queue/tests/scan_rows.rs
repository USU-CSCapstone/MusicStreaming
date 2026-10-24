//! Scan rows the store is slow to write, or cannot write: the queue answers while a request
//! writes, requests racing for the same work queue it once, and a request the store cannot record
//! is dropped (`design/scanning.md` §9).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use super::super::*;
use crate::scan::ScanState;
use crate::store::StoreError;
use crate::testing::{BareStore, Hooked, library, make_album, wait_until};

/// How long a test waits for something another thread does.
const BOUND: Duration = Duration::from_secs(10);

/// What the next scan-row writes of a store made by [`gated`] do, and the channels a held write
/// signals on and waits on.
struct Gate {
    /// Holds up the write this many writes from now, counting the next as 1, until released.
    hold_at: AtomicUsize,
    /// Fails the write this many writes from now.
    fail_at: AtomicUsize,
    enter: Sender<()>,
    entered: Mutex<Receiver<()>>,
    release: Sender<()>,
    released: Mutex<Receiver<()>>,
}

impl Gate {
    /// Holds up the `nth` scan-row write from now.
    fn hold(&self, nth: usize) {
        self.hold_at.store(nth, Ordering::SeqCst);
    }

    /// Fails the `nth` scan-row write from now.
    fn fail(&self, nth: usize) {
        self.fail_at.store(nth, Ordering::SeqCst);
    }

    /// Waits until a write is held up, failing the test after [`BOUND`].
    fn wait_held(&self) {
        let entered = self.entered.lock().unwrap().recv_timeout(BOUND);
        entered.expect("no scan-row write was held up");
    }

    /// Lets the held write go on.
    fn release(&self) {
        self.release.send(()).unwrap();
    }

    /// The store's hook on each scan-row write: held up or failed as set, else let through.
    fn write(&self) -> Result<(), StoreError> {
        // Counts `at` down, and says whether this write is the one it named.
        let reached = |at: &AtomicUsize| {
            at.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)) == Ok(1)
        };
        let (held, failed) = (reached(&self.hold_at), reached(&self.fail_at));
        if held {
            self.enter.send(()).unwrap();
            self.released.lock().unwrap().recv().unwrap();
        }
        if failed {
            return Err(StoreError("disk full".into()));
        }
        Ok(())
    }
}

/// A store whose scan-row writes the returned [`Gate`] holds up or fails, and the bare store it
/// wraps, which holds the rows.
fn gated() -> (Arc<Hooked<'static>>, Arc<BareStore>, Arc<Gate>) {
    let ((enter, entered), (release, released)) = (mpsc::channel(), mpsc::channel());
    let gate = Arc::new(Gate {
        hold_at: AtomicUsize::new(0),
        fail_at: AtomicUsize::new(0),
        enter,
        entered: Mutex::new(entered),
        release,
        released: Mutex::new(released),
    });
    let rows = Arc::new(BareStore::default());
    let hook = gate.clone();
    let store = Hooked::new(rows.clone()).on_scan_write(move || hook.write());
    (Arc::new(store), rows, gate)
}

/// A scanner of the library at `root`, A/X and B/Y with one track each, over `store`, running a
/// scan of A that `store` holds once its row, one of `rows`, says it runs, with a scan of B
/// queued behind it. Returns the scanner, the owner of its thread, and the two scans' ids.
fn parked(
    root: &Path,
    store: &Arc<Hooked<'static>>,
    rows: &BareStore,
) -> (Scanner, ScannerThread, ScanId, ScanId) {
    make_album(root, "A", "X", 1);
    make_album(root, "B", "Y", 1);
    store.hold();
    let (scanner, thread) = Scanner::start(store.clone(), library(root)).unwrap();
    let running = scanner.request(Trigger::Manual, folder(root, "A")).unwrap();
    let queued = scanner.request(Trigger::Manual, folder(root, "B")).unwrap();
    let saved_running =
        || rows.scans().iter().any(|s| s.id == running && s.state == ScanState::Running);
    wait_until("the scan of A never started", saved_running);
    (scanner, thread, running, queued)
}

/// A scan of the folder `name` under `root`, and all below it.
fn folder(root: &Path, name: &str) -> Vec<Scope> {
    vec![Scope::folder(root, root.join(name), Depth::Subtree)]
}

/// Runs `call` on `scanner` in a thread of its own.
fn in_thread<T: Send + 'static>(
    scanner: &Scanner,
    call: impl FnOnce(&Scanner) -> T + Send + 'static,
) -> thread::JoinHandle<T> {
    let scanner = scanner.clone();
    thread::spawn(move || call(&scanner))
}

/// The id of the scan `scanner` says is running, unless it does not answer within [`BOUND`].
fn running_within_bound(scanner: &Scanner) -> Result<Option<ScanId>, RecvTimeoutError> {
    let (answer, answered) = mpsc::channel();
    let scanner = scanner.clone();
    thread::spawn(move || answer.send(scanner.running().map(|scan| scan.id)));
    answered.recv_timeout(BOUND)
}

/// The state of the scan `id` among `rows`.
fn state(rows: &BareStore, id: ScanId) -> ScanState {
    rows.scans().into_iter().find(|s| s.id == id).unwrap().state
}

/// While a request waits on the store to write a scan row, its own or that of a queued scan it
/// replaces, the queue still says what runs: the write holds up only the call making it.
#[test]
fn the_queue_answers_while_a_scan_row_is_written() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let (store, rows, gate) = gated();
    let (scanner, thread, running, queued) = parked(&root, &store, &rows);
    gate.hold(1);
    let scopes = folder(&root, "C");
    let requesting = in_thread(&scanner, move |s| s.request(Trigger::Watch, scopes));
    gate.wait_held();
    assert_eq!(running_within_bound(&scanner), Ok(Some(running)));
    gate.release();
    let requested = requesting.join().unwrap().unwrap();

    // The new scan's row first, then the first replaced scan's, held up.
    gate.hold(2);
    let replacing = in_thread(&scanner, |s| s.scan_library(Trigger::Manual));
    gate.wait_held();
    assert_eq!(running_within_bound(&scanner), Ok(Some(running)));
    gate.release();
    let library = replacing.join().unwrap().unwrap();

    store.release();
    scanner.wait_idle();
    drop(thread);
    let states = [running, queued, requested, library].map(|id| state(&rows, id));
    use ScanState::{Cancelled, Completed};
    assert_eq!(states, [Completed, Cancelled, Cancelled, Completed]);
}

/// Two requests for the same work at once queue it once: the one whose row is written first
/// queues its scan, and the other, finding it queued once its own row is written, cancels that
/// row and returns the queued scan.
#[test]
fn requests_racing_for_the_same_work_queue_it_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let (store, rows, gate) = gated();
    let (scanner, thread, _, _) = parked(&root, &store, &rows);

    gate.hold(1);
    let scopes = folder(&root, "C");
    let first = in_thread(&scanner, move |s| s.request(Trigger::Watch, scopes));
    gate.wait_held();
    let second = scanner.request(Trigger::Watch, folder(&root, "C")).unwrap();
    gate.release();
    assert_eq!(first.join().unwrap(), Some(second));
    let queued = scanner.queued();
    assert_eq!(queued.iter().filter(|s| s.scopes == folder(&root, "C")).count(), 1);
    let rows = rows.scans();
    assert_eq!(rows.iter().filter(|s| s.state == ScanState::Cancelled).count(), 1);
    store.release();
    scanner.wait_idle();
    drop(thread);
}

/// A request a queued scan already covers joins it without writing a row.
#[test]
fn a_request_joining_a_queued_scan_writes_no_row() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let (store, rows, _) = gated();
    let (scanner, thread, _, queued) = parked(&root, &store, &rows);
    let written = rows.scans().len();

    let within = vec![Scope::folder(&root, root.join("B/Y"), Depth::Directory)];
    assert_eq!(scanner.request(Trigger::Watch, within), Some(queued));
    assert_eq!(rows.scans().len(), written);
    store.release();
    scanner.wait_idle();
    drop(thread);
}

/// A request the store cannot record is not queued and is not retried: its work waits for the
/// next trigger.
#[test]
fn a_request_the_store_cannot_record_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let (store, rows, gate) = gated();
    let options = ScanOptions { retry_wait: Duration::ZERO, ..Default::default() };
    let (scanner, thread) = Scanner::start_with(store.clone(), library(&root), options).unwrap();

    gate.fail(1);
    assert_eq!(scanner.scan_library(Trigger::Manual), None);
    thread::sleep(Duration::from_millis(200));
    assert!(scanner.is_idle());
    assert!(rows.scans().is_empty());
    drop(thread);
}
