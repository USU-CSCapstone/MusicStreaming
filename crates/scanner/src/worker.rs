//! A background thread that runs until its [`Worker`] is dropped.

use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A named background thread. Dropping it asks the thread to stop, through its [`Stop`], and
/// waits for it to finish.
#[must_use = "dropping a worker stops its thread"]
pub struct Worker {
    /// The sender of the thread's [`Stop`], and the thread; `None` once it is stopped.
    running: Option<(Sender<()>, JoinHandle<()>)>,
}

/// How a worker's thread learns that it should stop: its worker drops the sender of a channel
/// on which nothing is ever sent.
pub struct Stop(Receiver<()>);

impl Stop {
    /// Waits up to `timeout`, returning early once the worker is asked to stop. Returns whether
    /// it has been.
    pub fn wait(&self, timeout: Duration) -> bool {
        matches!(self.0.recv_timeout(timeout), Err(RecvTimeoutError::Disconnected))
    }
}

impl Worker {
    /// Runs `body` on a thread named `name`, with the [`Stop`] that tells it when to return. A
    /// panic in `body` ends the thread and is logged there as it happens, since a join might
    /// report it late or never: the owner joins only when it drops the worker, and not at all
    /// while it unwinds.
    pub fn spawn<F>(name: &str, body: F) -> io::Result<Worker>
    where
        F: FnOnce(Stop) + Send + 'static,
    {
        let (stop, stopping) = mpsc::channel();
        let thread = thread::Builder::new().name(name.into()).spawn(move || {
            if panic::catch_unwind(AssertUnwindSafe(|| body(Stop(stopping)))).is_err() {
                panicked();
            }
        })?;
        Ok(Worker { running: Some((stop, thread)) })
    }

    /// Asks the thread to stop and waits for it to finish. While this thread is panicking it
    /// does not wait, so an unwind is never held up by a thread that may not finish, such as one
    /// blocked on what the unwind would have released; that thread is left to end alone. Does
    /// nothing once the thread is stopped.
    pub(crate) fn stop(&mut self) {
        let Some((stop, thread)) = self.running.take() else {
            return;
        };
        drop(stop);
        if thread::panicking() {
            return;
        }
        // The thread logs a panic in its body itself; this one can only come from that logging.
        if thread.join().is_err() {
            panicked();
        }
    }
}

impl Drop for Worker {
    /// [`Worker::stop`].
    fn drop(&mut self) {
        self.stop();
    }
}

/// Logs that the current background thread panicked.
fn panicked() {
    tracing::error!(thread = thread::current().name(), "a background thread panicked");
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// Longer than any test waits for a stop.
    const LONG: Duration = Duration::from_secs(60);

    /// A worker waiting out a long timeout stops as soon as it is dropped, and its wait says so.
    #[test]
    fn a_dropped_worker_stops_at_once() {
        let (waiting, waited) = mpsc::channel();
        let worker = Worker::spawn("test-waiting", move |stop| {
            let early = stop.wait(Duration::ZERO);
            waiting.send(early).unwrap();
            waiting.send(stop.wait(LONG)).unwrap();
        })
        .unwrap();
        assert!(!waited.recv().unwrap(), "asked to stop before the worker was dropped");
        let stopping = Instant::now();
        drop(worker);
        assert!(stopping.elapsed() < LONG / 4, "the stop took {:?}", stopping.elapsed());
        assert!(waited.recv().unwrap(), "the wait timed out rather than seeing the stop");
    }

    /// A worker dropped as its owner unwinds is not waited for, so a failed assertion while its
    /// thread is blocked, here on a channel nothing will send to, fails rather than hangs.
    #[test]
    fn an_unwind_does_not_wait_for_a_worker() {
        let (release, released) = mpsc::channel::<()>();
        let unwinding = Instant::now();
        let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
            let _blocked = Worker::spawn("test-blocked", move |_| {
                assert!(released.recv_timeout(LONG).is_err(), "nothing is sent");
            })
            .unwrap();
            panic!("a failed assertion");
        }));
        assert!(unwound.is_err());
        assert!(unwinding.elapsed() < LONG / 4, "the unwind took {:?}", unwinding.elapsed());
        drop(release);
    }
}
