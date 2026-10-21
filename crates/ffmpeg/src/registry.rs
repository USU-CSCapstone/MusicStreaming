//! The process pool: who is running, who has stalled, and pausing them all.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};

/// How long a child may go without progress before the watchdog kills it. A PCM decode makes
/// progress with every read of its output; a probe or an image decode records none, so for them
/// this bounds the whole run.
pub(crate) const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The most children the pool admits at once: a target rather than a hard bound (see
/// `Registry::acquire`).
const MAX_PROCESSES: usize = 2;

struct Entry {
    last_progress: Instant,
    /// Set when the watchdog killed it, so the reader can report a stall
    /// rather than a generic failure.
    stalled: Arc<AtomicBool>,
}

struct State {
    children: HashMap<Pid, Entry>,
    paused: bool,
}

/// Tracks every live ffmpeg child so the watchdog can kill stalls and the
/// governor can stop and continue them with signals.
pub struct Registry {
    state: Mutex<State>,
    /// Signalled when a slot frees or the pool is resumed.
    slot_free: Condvar,
}

impl Registry {
    /// Create an empty pool and start its watchdog thread, which checks for stalls every second
    /// until the registry is dropped.
    pub(crate) fn start() -> Arc<Registry> {
        let registry = Arc::new(Registry {
            state: Mutex::new(State { children: HashMap::new(), paused: false }),
            slot_free: Condvar::new(),
        });
        let weak = Arc::downgrade(&registry);
        std::thread::Builder::new()
            .name("ffmpeg-watchdog".into())
            .spawn(move || {
                while let Some(reg) = weak.upgrade() {
                    reg.reap_stalled();
                    drop(reg);
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .expect("spawn ffmpeg watchdog");
        registry
    }

    /// Block until a pool slot is free and the pool is not paused, then take it.
    pub(crate) fn acquire(&self) {
        let mut state = self.state.lock().unwrap();
        while state.paused || state.children.len() >= MAX_PROCESSES {
            state = self.slot_free.wait(state).unwrap();
        }
        // The slot is claimed by inserting the child in `register`; between
        // here and there the caller holds no lock, so over-admission by one is
        // possible under contention. Acceptable: the bound is a target, not a
        // correctness property.
    }

    /// Add the child `pid` to the pool, stopped at once if the pool is paused. Returns the flag the
    /// watchdog sets if it kills the child as stalled. The caller must [`Registry::unregister`]
    /// the child before reaping it, as [`crate::pooled::Pooled`] does.
    pub(crate) fn register(&self, pid: Pid) -> Arc<AtomicBool> {
        let stalled = Arc::new(AtomicBool::new(false));
        let mut state = self.state.lock().unwrap();
        state
            .children
            .insert(pid, Entry { last_progress: Instant::now(), stalled: stalled.clone() });
        if state.paused {
            send(pid, Signal::STOP);
        }
        stalled
    }

    /// Record that the child `pid` made progress, restarting its stall clock.
    pub(crate) fn progress(&self, pid: Pid) {
        if let Some(entry) = self.state.lock().unwrap().children.get_mut(&pid) {
            entry.last_progress = Instant::now();
        }
    }

    /// Remove the child `pid` from the pool, freeing its slot. After this returns, nothing here
    /// signals that pid again.
    pub(crate) fn unregister(&self, pid: Pid) {
        self.state.lock().unwrap().children.remove(&pid);
        self.slot_free.notify_all();
    }

    /// Kill every child that has gone [`STALL_TIMEOUT`] without progress, marking it stalled.
    fn reap_stalled(&self) {
        let state = self.state.lock().unwrap();
        if state.paused {
            return; // a stopped process is not a stalled one
        }
        let now = Instant::now();
        for (pid, entry) in &state.children {
            if now.duration_since(entry.last_progress) > STALL_TIMEOUT {
                tracing::warn!(pid = pid.as_raw_pid(), "ffmpeg stalled; killing");
                entry.stalled.store(true, Ordering::SeqCst);
                send(*pid, Signal::KILL);
            }
        }
    }

    /// Stop every child and admit no new ones. A paused decode resumes where
    /// it was; nothing restarts (`design/scanning.md` §10).
    pub fn pause_all(&self) {
        let mut state = self.state.lock().unwrap();
        state.paused = true;
        for pid in state.children.keys() {
            send(*pid, Signal::STOP);
        }
    }

    /// Continue every child and reopen admission. Progress clocks are reset so
    /// time spent stopped does not count as a stall.
    pub fn resume_all(&self) {
        let mut state = self.state.lock().unwrap();
        state.paused = false;
        let now = Instant::now();
        for (pid, entry) in state.children.iter_mut() {
            entry.last_progress = now;
            send(*pid, Signal::CONT);
        }
        drop(state);
        self.slot_free.notify_all();
    }
}

/// Send `signal` to the registered child `pid`, logging a failure. Callers hold the registry's
/// lock, which is what lets [`Registry::unregister`] promise no signal once it returns.
fn send(pid: Pid, signal: Signal) {
    if let Err(error) = kill_process(pid, signal) {
        tracing::warn!(pid = pid.as_raw_pid(), ?signal, %error, "could not signal ffmpeg");
    }
}

#[cfg(test)]
mod tests;
