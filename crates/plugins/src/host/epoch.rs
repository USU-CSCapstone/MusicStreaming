//! The engine's epoch, which every run's deadlines and compute budget count in (`design/plugins.md`
//! §2): advanced every [`TICK`] while a run is live and not at all otherwise, so a server whose
//! plugins are idle never wakes for it.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

/// How often the epoch advances, and so how often a running plugin is checked. A tick is a sleep
/// that runs a little long, at about 1.05 ms.
const TICK: Duration = Duration::from_millis(1);

/// Advances the epoch, from a thread of its own, while any run holds a [`Live`]; the thread
/// sleeps while none does. Dropping it ends the thread.
pub(super) struct Epoch(Arc<Shared>);

/// A run's hold on the epoch, which advances while any is held.
pub(super) struct Live(Arc<Shared>);

/// What the thread shares with the [`Epoch`] and each [`Live`].
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// Signalled when a run goes live and when the [`Epoch`] is dropped.
    wake: Condvar,
}

/// Whether the thread has a run to advance the epoch for, or should end.
#[derive(Default)]
struct State {
    /// How many [`Live`]s are held.
    live: usize,
    /// Whether the [`Epoch`] is dropped, which ends the thread.
    closed: bool,
}

impl Epoch {
    /// Starts the thread, which calls `advance` every [`TICK`] while a run is live. Fails if the
    /// thread cannot be started.
    pub fn start(mut advance: impl FnMut() + Send + 'static) -> std::io::Result<Epoch> {
        let shared = Arc::new(Shared::default());
        let ticker = shared.clone();
        thread::Builder::new().name("plugin-epoch".into()).spawn(move || {
            while ticker.wait_for_a_run() {
                advance();
                thread::sleep(TICK);
            }
        })?;
        Ok(Epoch(shared))
    }

    /// Keeps the epoch advancing until the answer is dropped. The thread looks for a live run
    /// under the lock this takes, and sleeps only by releasing it, so it either sees this run
    /// or is asleep and woken by it.
    pub fn live(&self) -> Live {
        self.0.lock().live += 1;
        self.0.wake.notify_one();
        Live(self.0.clone())
    }
}

impl Shared {
    /// The state, locked.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("epoch lock")
    }

    /// Sleeps until a run is live or the [`Epoch`] is dropped, and answers whether a run is.
    fn wait_for_a_run(&self) -> bool {
        let idle = |state: &mut State| state.live == 0 && !state.closed;
        let state = self.wake.wait_while(self.lock(), idle).expect("epoch lock");
        !state.closed
    }
}

impl Drop for Live {
    /// Ends this run's hold; the thread sleeps at its next check if it was the last.
    fn drop(&mut self) {
        self.0.lock().live -= 1;
    }
}

impl Drop for Epoch {
    /// Tells the thread to end, waking it if it sleeps. It ends after its current tick, without
    /// waiting for any [`Live`] still held.
    fn drop(&mut self) {
        self.0.lock().closed = true;
        self.0.wake.notify_one();
    }
}

#[cfg(test)]
mod tests;
