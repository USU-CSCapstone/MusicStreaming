//! Scan rows the store is slow to write, or cannot write: the queue answers while a request or
//! cancel writes, requests racing for the same work queue it once, a request the store cannot
//! record is retried, and a cancel the store cannot save leaves its scan queued where it was
//! (`design/scanning.md` §9).

use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use super::*;
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
        |_: &Scanner| rows.scans().iter().any(|s| s.id == running && s.state == ScanState::Running);
    wait_until(&scanner, "the scan of A never started", saved_running);
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

/// While a request, a cancel, or a request replacing a queued scan waits on the store to write a
/// scan row, the queue still says what runs: the write holds up only the call making it.
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

    gate.hold(1);
    let cancelling = in_thread(&scanner, move |s| s.cancel(queued));
    gate.wait_held();
    assert_eq!(running_within_bound(&scanner), Ok(Some(running)));
    gate.release();
    assert!(cancelling.join().unwrap());

    // The new scan's row first, then the replaced scan's, held up.
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

/// A request the store cannot record waits to be queued again, as a suspended scan's work does,
/// and once recorded runs.
#[test]
fn a_request_the_store_cannot_record_is_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let (store, rows, gate) = gated();
    let (scanner, thread) = Scanner::start(store.clone(), library(&root)).unwrap();

    gate.fail(1);
    assert_eq!(scanner.scan_library(Trigger::Manual), None);
    assert!(scanner.retry_in(Duration::ZERO), "the request waits to be retried");
    let ended = |_: &Scanner| rows.scans().first().is_some_and(|s| s.finished_at.is_some());
    wait_until(&scanner, "the retry never ended", ended);
    drop(thread);
    let [row] = rows.scans().try_into().unwrap();
    assert_eq!((row.trigger, row.state), (Trigger::Manual, ScanState::Completed));
    assert_eq!(row.scopes, [Scope::root(&root)]);
}

/// A queued scan whose row the store cannot save cancelled is not cancelled: it stays queued,
/// between the scans queued before and after it, and runs.
#[test]
fn a_cancel_the_store_cannot_save_leaves_the_scan_queued_where_it_was() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let (store, rows, gate) = gated();
    let (scanner, thread, running, queued) = parked(&root, &store, &rows);
    let c = scanner.request(Trigger::Manual, folder(&root, "C")).unwrap();
    let d = scanner.request(Trigger::Manual, folder(&root, "D")).unwrap();

    gate.fail(1);
    assert!(!scanner.cancel(c));
    let ids: Vec<ScanId> = scanner.queued().iter().map(|s| s.id).collect();
    assert_eq!(ids, [queued, c, d]);
    store.release();
    scanner.wait_idle();
    drop(thread);
    let states = [running, queued, c, d].map(|id| state(&rows, id));
    assert_eq!(states, [ScanState::Completed; 4]);
}

/// A scan put back after a failed cancel goes after the last scan still queued that was ahead
/// of it, though one ahead of it left the queue while the cancel was writing.
#[test]
fn a_scan_put_back_goes_after_what_is_still_ahead_of_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let (store, rows, gate) = gated();
    let (scanner, thread, _, queued) = parked(&root, &store, &rows);
    let c = scanner.request(Trigger::Manual, folder(&root, "C")).unwrap();
    let d = scanner.request(Trigger::Manual, folder(&root, "D")).unwrap();

    gate.hold(1);
    gate.fail(1);
    let cancelling = in_thread(&scanner, move |s| s.cancel(c));
    gate.wait_held();
    assert!(scanner.cancel(queued));
    gate.release();
    assert!(!cancelling.join().unwrap());
    let ids: Vec<ScanId> = scanner.queued().iter().map(|s| s.id).collect();
    assert_eq!(ids, [c, d]);
    store.release();
    scanner.wait_idle();
    drop(thread);
}

/// A scan put back after a failed cancel runs, though the worker had found the queue empty and
/// gone to wait while the cancel was writing.
#[test]
fn a_scan_put_back_after_a_failed_cancel_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let (store, rows, gate) = gated();
    let (scanner, thread, running, queued) = parked(&root, &store, &rows);

    gate.hold(1);
    gate.fail(1);
    let cancelling = in_thread(&scanner, move |s| s.cancel(queued));
    gate.wait_held();
    store.release();
    let idle = |s: &Scanner| s.is_idle() && state(&rows, running) == ScanState::Completed;
    wait_until(&scanner, "the running scan never ended", idle);
    gate.release();
    assert!(!cancelling.join().unwrap());
    let ran = |_: &Scanner| state(&rows, queued) == ScanState::Completed;
    wait_until(&scanner, "the scan put back never ran", ran);
    drop(thread);
}
