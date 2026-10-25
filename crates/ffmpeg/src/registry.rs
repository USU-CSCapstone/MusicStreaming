//! The process pool: every background ffmpeg child, and the watchdog that kills those that
//! stall. The scanner's pool bounds how many run at once: each of its threads runs one child at
//! a time.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};

use jewelcase_core::Recover;

/// How long a child may go without progress, by the wall clock, before the watchdog kills it. A
/// PCM decode makes progress with every read of its output; a probe or an image decode records
/// none, so for them this bounds the whole run.
pub(crate) const STALL_TIMEOUT: Duration = Duration::from_secs(120);

/// How often the watchdog checks for stalls.
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// What the watchdog keeps for one child.
struct Entry {
    /// When the child registered, or last made progress.
    last_progress: Instant,
    /// Set when the watchdog killed it, so the reader can report a stall
    /// rather than a generic failure.
    stalled: Arc<AtomicBool>,
}

/// Tracks every live background ffmpeg child so the watchdog can kill stalls, and so the server
/// can kill them all as it stops.
#[derive(Default)]
pub(crate) struct Registry {
    children: Mutex<HashMap<Pid, Entry>>,
    /// Set by [`Registry::stop`], under the lock on `children`.
    stopping: AtomicBool,
}

impl Registry {
    /// Create an empty pool and start its watchdog thread, which checks for stalls every second
    /// until the registry is dropped. Fails if the thread can't start.
    pub(crate) fn start() -> io::Result<Arc<Registry>> {
        let registry = Arc::new(Registry::default());
        let weak = Arc::downgrade(&registry);
        std::thread::Builder::new().name("ffmpeg-watchdog".into()).spawn(move || {
            loop {
                std::thread::sleep(CHECK_EVERY);
                let Some(registry) = weak.upgrade() else { break };
                registry.reap_stalled(Instant::now());
            }
        })?;
        Ok(registry)
    }

    /// Add the child `pid` to the pool. Returns the flag the watchdog sets if it kills the child
    /// as stalled. The caller must [`Registry::unregister`] the child before reaping it, as
    /// [`crate::pooled::Pooled`] does.
    pub(crate) fn register(&self, pid: Pid) -> Arc<AtomicBool> {
        let stalled = Arc::new(AtomicBool::new(false));
        let entry = Entry { last_progress: Instant::now(), stalled: stalled.clone() };
        let mut children = self.children.lock().recover();
        // Checked under the lock `stop` holds, so a child that registers as it runs is killed
        // by one or the other.
        if self.stopping() {
            send(pid, Signal::KILL);
        }
        children.insert(pid, entry);
        stalled
    }

    /// Kills every child, and every child that registers afterwards as it registers, for the
    /// server's stop.
    pub(crate) fn stop(&self) {
        let children = self.children.lock().recover();
        self.stopping.store(true, Ordering::SeqCst);
        for &pid in children.keys() {
            send(pid, Signal::KILL);
        }
    }

    /// Whether [`Registry::stop`] has run.
    pub(crate) fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// Record that the child `pid` made progress, restarting its stall clock.
    pub(crate) fn progress(&self, pid: Pid) {
        if let Some(entry) = self.children.lock().recover().get_mut(&pid) {
            entry.last_progress = Instant::now();
        }
    }

    /// Remove the child `pid` from the pool. After this returns, nothing here signals that pid
    /// again.
    pub(crate) fn unregister(&self, pid: Pid) {
        self.children.lock().recover().remove(&pid);
    }

    /// Kill every child that has made no progress for longer than [`STALL_TIMEOUT`] as of `now`,
    /// marking it stalled.
    fn reap_stalled(&self, now: Instant) {
        for (pid, entry) in self.children.lock().recover().iter() {
            if now.saturating_duration_since(entry.last_progress) > STALL_TIMEOUT {
                tracing::warn!(pid = pid.as_raw_pid(), "ffmpeg stalled; killing");
                entry.stalled.store(true, Ordering::SeqCst);
                send(*pid, Signal::KILL);
            }
        }
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
