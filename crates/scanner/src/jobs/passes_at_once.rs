//! Passes of one job running at once, on the pool's threads: each takes files the others do not
//! have in hand.

use std::sync::mpsc;
use std::thread;

use super::tests::setup;
use super::*;
use crate::testing::BareStore;

/// Longer than any test waits for another thread.
const LONG: Duration = Duration::from_secs(5);

/// A queue standing in for the store's: a pass fetches from its front, and a file leaves it once
/// its result is kept. Every decode is recorded.
struct Queue {
    files: Mutex<Vec<(usize, FileToDecode)>>,
    decoded: Mutex<Vec<PathBuf>>,
}

impl Queue {
    /// A queue of `files`, in order.
    fn of(files: &[FileToDecode]) -> Queue {
        let files = files.iter().cloned().enumerate().collect();
        Queue { files: Mutex::new(files), decoded: Mutex::default() }
    }

    /// The first `fetch` files.
    fn next(&self, fetch: usize) -> Result<Vec<(usize, FileToDecode)>, StoreError> {
        Ok(self.files.lock().unwrap().iter().take(fetch).cloned().collect())
    }

    /// Records a decode of `file`.
    fn decode(&self, file: &FileToDecode) -> Result<(), Failure> {
        self.decoded.lock().unwrap().push(file.path.clone());
        Ok(())
    }

    /// Takes the file `key` off the queue.
    fn keep(&self, key: usize) -> Result<(), StoreError> {
        self.files.lock().unwrap().retain(|(queued, _)| *queued != key);
        Ok(())
    }

    /// Every decode, sorted.
    fn decoded(&self) -> Vec<PathBuf> {
        let mut decoded = self.decoded.lock().unwrap().clone();
        decoded.sort();
        decoded
    }
}

/// Runs a pass of one file over `queue` on another thread, holding up its decode until `finish`,
/// the sender returned, is sent to. Returns once the decode has started.
fn held_pass<'s>(
    scope: &'s thread::Scope<'s, '_>,
    retries: &'s Retries,
    store: &'s BareStore,
    queue: &'s Queue,
) -> (mpsc::Sender<()>, thread::ScopedJoinHandle<'s, RunReport>) {
    let (started, decoding) = mpsc::channel();
    let (finish, may_finish) = mpsc::channel::<()>();
    let pass = scope.spawn(move || {
        let decode = |file: &FileToDecode| {
            queue.decode(file)?;
            started.send(()).unwrap();
            may_finish.recv_timeout(LONG).unwrap();
            Ok(())
        };
        let keep = |key, _| queue.keep(key);
        retries.pass(store, 1, 1, |fetch| queue.next(fetch), decode, keep).unwrap()
    });
    decoding.recv_timeout(LONG).unwrap();
    (finish, pass)
}

/// A pass running beside another skips the file the other has in hand.
#[test]
fn passes_at_once_take_different_files() {
    let (_tmp, store, [a, b]) = setup();
    let retries = Retries::new(RETRY_AFTER);
    let queue = Queue::of(&[a.clone(), b.clone()]);
    thread::scope(|scope| {
        let (finish, first) = held_pass(scope, &retries, &store, &queue);
        let keep = |key, _| queue.keep(key);
        let second = retries.pass(&store, 1, 1, |n| queue.next(n), |f| queue.decode(f), keep);
        assert_eq!(second.unwrap().done, 1);
        finish.send(()).unwrap();
        assert_eq!(first.join().unwrap().done, 1);
    });
    assert_eq!(queue.decoded(), [a.path, b.path]);
}

/// A pass that fetched a file before another pass kept its result does not take the file once
/// that pass is done with it: a file is given back only while no pass is fetching.
#[test]
fn a_fetch_from_before_a_result_is_kept_does_not_take_its_file() {
    let (_tmp, store, [a, b]) = setup();
    let retries = Retries::new(RETRY_AFTER);
    let queue = Queue::of(&[a.clone(), b.clone()]);
    thread::scope(|scope| {
        let (finish, first) = held_pass(scope, &retries, &store, &queue);
        let (fetched, has_fetched) = mpsc::channel();
        let (take, may_take) = mpsc::channel::<()>();
        let (retries, store, queue) = (&retries, &store, &queue);
        let second = scope.spawn(move || {
            let next = |fetch| {
                let files = queue.next(fetch);
                fetched.send(()).unwrap();
                may_take.recv_timeout(LONG).unwrap();
                files
            };
            let keep = |key, _| queue.keep(key);
            retries.pass(store, 1, 1, next, |f| queue.decode(f), keep).unwrap()
        });
        has_fetched.recv_timeout(LONG).unwrap();
        finish.send(()).unwrap();
        // Time for the first pass to keep its result and give its file back, were it free to.
        thread::sleep(Duration::from_millis(100));
        take.send(()).unwrap();
        assert_eq!(second.join().unwrap().done, 1);
        assert_eq!(first.join().unwrap().done, 1);
    });
    assert_eq!(queue.decoded(), [a.path, b.path]);
}

/// A pass gives back every file it took: those it never reached, and the one a store error ended
/// it at.
#[test]
fn a_pass_gives_back_every_file_it_took() {
    let (_tmp, store, [a, b]) = setup();
    let retries = Retries::new(RETRY_AFTER);
    let queue = Queue::of(&[a, b]);
    let keep = |key, _| queue.keep(key);
    let report = retries.pass(&store, 1, 1, |n| queue.next(n), |f| queue.decode(f), keep);
    assert_eq!(report.unwrap().done, 1);
    assert!(retries.in_hand().is_empty());
    let failing = |_, _| Err(StoreError("down".into()));
    let pass = retries.pass(&store, 1, 1, |n| queue.next(n), |f| queue.decode(f), failing);
    assert!(pass.is_err());
    assert!(retries.in_hand().is_empty());
}

/// A pass takes only its share of what it fetched: here one file, though the file another pass
/// holds has already left the queue, so the fetch it made room for returns one free file more.
#[test]
fn a_pass_takes_only_its_share() {
    let (_tmp, store, [a, b]) = setup();
    let retries = Retries::new(RETRY_AFTER);
    retries.in_hand().insert(PathBuf::from("/kept/and/not/yet/given/back"));
    let queue = Queue::of(&[a, b]);
    let decode = |file: &FileToDecode| {
        assert_eq!(retries.in_hand().len(), 2, "taken beyond the share");
        queue.decode(file)
    };
    let keep = |key, _| queue.keep(key);
    let report = retries.pass(&store, 1, 1, |n| queue.next(n), decode, keep).unwrap();
    assert_eq!(report.done, 1);
}
