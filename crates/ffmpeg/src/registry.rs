//! The process pool: every background ffmpeg child, the slots that bound how many run at once,
//! and the watchdog that kills those that stall.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};

/// How long a child may go without progress before the watchdog kills it, counted as
/// [`Registry::reap_stalled`] says. A PCM decode makes progress with every read of its output; a
/// probe or an image decode records none, so for them this bounds the whole run.
pub(crate) const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the watchdog checks for stalls.
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// The most background children that run at once, across every library that shares the pool.
/// Each holds a decode's memory, which for a 64-megapixel picture is about 400 MB.
const MAX_CHILDREN: usize = 2;

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

/// Tracks every live background ffmpeg child so the watchdog can kill stalls, and bounds how
/// many run at once.
#[derive(Default)]
pub(crate) struct Registry {
    children: Mutex<HashMap<Pid, Entry>>,
    /// How many [`Slot`]s are taken.
    taken: Mutex<usize>,
    /// Signalled when a slot is freed.
    freed: Condvar,
}

/// One of the pool's [`MAX_CHILDREN`] places, held by a [`crate::pooled::Pooled`] child. Dropping
/// it frees the place.
pub(crate) struct Slot(Arc<Registry>);

impl Drop for Slot {
    /// Frees the place and wakes one caller waiting for it.
    fn drop(&mut self) {
        *self.0.taken.lock().unwrap() -= 1;
        self.0.freed.notify_one();
    }
}

impl Registry {
    /// Create an empty pool and start its watchdog thread, which checks for stalls every second
    /// until the registry is dropped.
    pub(crate) fn start() -> Arc<Registry> {
        let registry = Arc::new(Registry::default());
        let weak = Arc::downgrade(&registry);
        std::thread::Builder::new()
            .name("ffmpeg-watchdog".into())
            .spawn(move || {
                let mut checked = Instant::now();
                loop {
                    std::thread::sleep(CHECK_EVERY);
                    let Some(registry) = weak.upgrade() else { break };
                    let now = Instant::now();
                    registry.reap_stalled(now - checked, sample);
                    checked = now;
                }
            })
            .expect("spawn ffmpeg watchdog");
        registry
    }

    /// Wait until fewer than [`MAX_CHILDREN`] slots are taken, then take one.
    pub(crate) fn slot(self: &Arc<Self>) -> Slot {
        let taken = self.taken.lock().unwrap();
        let mut taken = self.freed.wait_while(taken, |taken| *taken >= MAX_CHILDREN).unwrap();
        *taken += 1;
        Slot(self.clone())
    }

    /// Add the child `pid` to the pool. Returns the flag the watchdog sets if it kills the child
    /// as stalled. The caller must [`Registry::unregister`] the child before reaping it, as
    /// [`crate::pooled::Pooled`] does.
    pub(crate) fn register(&self, pid: Pid) -> Arc<AtomicBool> {
        let entry = Entry::default();
        let stalled = entry.stalled.clone();
        self.children.lock().unwrap().insert(pid, entry);
        stalled
    }

    /// Record that the child `pid` made progress, restarting its stall clock.
    pub(crate) fn progress(&self, pid: Pid) {
        if let Some(entry) = self.children.lock().unwrap().get_mut(&pid) {
            entry.without_progress = Duration::ZERO;
        }
    }

    /// Remove the child `pid` from the pool. After this returns, nothing here signals that pid
    /// again.
    pub(crate) fn unregister(&self, pid: Pid) {
        self.children.lock().unwrap().remove(&pid);
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
        for (pid, entry) in self.children.lock().unwrap().iter_mut() {
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
