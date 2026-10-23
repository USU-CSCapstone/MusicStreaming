//! The process pool: every background ffmpeg child, and the watchdog that kills those that
//! stall. The scanner's pool bounds how many run at once: each of its threads runs one child at
//! a time.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};

use crate::lock::Recover;

/// How long a child may go without progress before the watchdog kills it, counted as
/// [`Registry::reap_stalled`] says. A PCM decode makes progress with every read of its output; a
/// probe or an image decode records none, so for them this bounds the whole run.
pub(crate) const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the watchdog checks for stalls.
const CHECK_EVERY: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Entry {
    /// How long the child has gone without progress, counted as [`Registry::reap_stalled`] says.
    without_progress: Duration,
    /// The CPU time the child had used when the watchdog last looked.
    cpu: Duration,
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
            let mut checked = Instant::now();
            loop {
                std::thread::sleep(CHECK_EVERY);
                let Some(registry) = weak.upgrade() else { break };
                let now = Instant::now();
                registry.reap_stalled(now - checked, sample);
                checked = now;
            }
        })?;
        Ok(registry)
    }

    /// Add the child `pid` to the pool. Returns the flag the watchdog sets if it kills the child
    /// as stalled. The caller must [`Registry::unregister`] the child before reaping it, as
    /// [`crate::pooled::Pooled`] does.
    pub(crate) fn register(&self, pid: Pid) -> Arc<AtomicBool> {
        let entry = Entry::default();
        let stalled = entry.stalled.clone();
        let mut children = self.children.lock().recover();
        // Checked under the lock `stop` holds, so a child that registers as it runs is killed
        // by one or the other.
        if self.stopping() {
            send(pid, Signal::KILL);
        }
        children.insert(pid, entry);
        stalled
    }

    /// Kills every child, and from now on every child as it registers, for the server's stop.
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
            entry.without_progress = Duration::ZERO;
        }
    }

    /// Remove the child `pid` from the pool. After this returns, nothing here signals that pid
    /// again.
    pub(crate) fn unregister(&self, pid: Pid) {
        self.children.lock().recover().remove(&pid);
    }

    /// Add to each child's time without progress, and kill every child whose time passes
    /// [`STALL_TIMEOUT`], marking it stalled. `elapsed` is the time since the last check, and
    /// `sample` reads a child's state and CPU time.
    ///
    /// A child that is running, or waiting for a CPU, or whose CPU time rose, adds the CPU time it
    /// used, up to `elapsed`, so one starved of the CPU by busier work is not taken for stalled.
    /// Any other child is blocked, and adds `elapsed`, so one blocked for good is killed. So does
    /// a child that cannot be sampled, as none can off Linux.
    fn reap_stalled(&self, elapsed: Duration, sample: impl Fn(Pid) -> io::Result<Sample>) {
        for (pid, entry) in self.children.lock().recover().iter_mut() {
            let added = match sample(*pid) {
                Ok(Sample { running, cpu }) => {
                    let used = cpu.saturating_sub(std::mem::replace(&mut entry.cpu, cpu));
                    if running || !used.is_zero() { used.min(elapsed) } else { elapsed }
                }
                Err(error) => {
                    tracing::debug!(pid = pid.as_raw_pid(), %error, "ffmpeg not sampled");
                    elapsed
                }
            };
            entry.without_progress += added;
            if entry.without_progress > STALL_TIMEOUT {
                tracing::warn!(pid = pid.as_raw_pid(), "ffmpeg stalled; killing");
                entry.stalled.store(true, Ordering::SeqCst);
                send(*pid, Signal::KILL);
            }
        }
    }
}

/// What the watchdog reads of a child.
#[derive(Debug, PartialEq)]
struct Sample {
    /// Whether its main thread is running or waiting for a CPU (state `R`). Other threads, such
    /// as a decoder's, show only in its CPU time.
    running: bool,
    /// The CPU time the process and all its threads have used.
    cpu: Duration,
}

/// Samples the process `pid` from `/proc/<pid>/stat`. Fails off Linux, which has no `/proc`.
fn sample(pid: Pid) -> io::Result<Sample> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid.as_raw_pid()))?;
    let (state, ticks) = parse_stat(&stat)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unexpected /proc/<pid>/stat"))?;
    let per_second = rustix::param::clock_ticks_per_second();
    let cpu = Duration::from_nanos(ticks.saturating_mul(1_000_000_000) / per_second);
    Ok(Sample { running: state == "R", cpu })
}

/// The state, field 3, of a `/proc/<pid>/stat` line, and its user and system times, fields 14
/// and 15 (`proc_pid_stat(5)`), added together, in clock ticks.
fn parse_stat(stat: &str) -> Option<(&str, u64)> {
    // The name, field 2, is in parentheses and may hold spaces and parentheses itself.
    let (_, fields) = stat.rsplit_once(") ")?;
    let mut fields = fields.split(' ');
    let state = fields.next()?;
    let mut times = fields.skip(10).map(str::parse::<u64>);
    Some((state, times.next()?.ok()? + times.next()?.ok()?))
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
