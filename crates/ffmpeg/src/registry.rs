//! The process pool: who is running, who has stalled, and pausing them all.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

struct Entry {
    last_progress: Instant,
    /// Set when the watchdog killed it, so the reader can report a stall
    /// rather than a generic failure.
    stalled: Arc<AtomicBool>,
}

struct State {
    children: HashMap<u32, Entry>,
    paused: bool,
}

/// Tracks every live ffmpeg child so the watchdog can kill stalls and the
/// governor can stop and continue them with signals.
pub struct Registry {
    state: Mutex<State>,
    /// Signalled when a slot frees or the pool is resumed.
    slot_free: Condvar,
    stall_timeout: Duration,
    max_processes: usize,
}

impl Registry {
    pub(crate) fn start(stall_timeout: Duration, max_processes: usize) -> Arc<Registry> {
        let registry = Arc::new(Registry {
            state: Mutex::new(State {
                children: HashMap::new(),
                paused: false,
            }),
            slot_free: Condvar::new(),
            stall_timeout,
            max_processes: max_processes.max(1),
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
        while state.paused || state.children.len() >= self.max_processes {
            state = self.slot_free.wait(state).unwrap();
        }
        // The slot is claimed by inserting the child in `register`; between
        // here and there the caller holds no lock, so over-admission by one is
        // possible under contention. Acceptable: the bound is a target, not a
        // correctness property.
    }

    pub(crate) fn register(&self, pid: u32) -> Arc<AtomicBool> {
        let stalled = Arc::new(AtomicBool::new(false));
        let mut state = self.state.lock().unwrap();
        state.children.insert(
            pid,
            Entry {
                last_progress: Instant::now(),
                stalled: stalled.clone(),
            },
        );
        if state.paused {
            // Born into a paused pool: stop it immediately.
            let _ = kill(Pid::from_raw(pid as i32), Signal::SIGSTOP);
        }
        stalled
    }

    pub(crate) fn progress(&self, pid: u32) {
        if let Some(entry) = self.state.lock().unwrap().children.get_mut(&pid) {
            entry.last_progress = Instant::now();
        }
    }

    pub(crate) fn unregister(&self, pid: u32) {
        self.state.lock().unwrap().children.remove(&pid);
        self.slot_free.notify_all();
    }

    fn reap_stalled(&self) {
        let state = self.state.lock().unwrap();
        if state.paused {
            return; // a stopped process is not a stalled one
        }
        let now = Instant::now();
        for (pid, entry) in &state.children {
            if now.duration_since(entry.last_progress) > self.stall_timeout {
                tracing::warn!(pid, "ffmpeg stalled; killing");
                entry.stalled.store(true, Ordering::SeqCst);
                let _ = kill(Pid::from_raw(*pid as i32), Signal::SIGKILL);
            }
        }
    }

    /// Stop every child and admit no new ones. A paused decode resumes where
    /// it was; nothing restarts (`design/scanning.md` §10).
    pub fn pause_all(&self) {
        let mut state = self.state.lock().unwrap();
        state.paused = true;
        for pid in state.children.keys() {
            let _ = kill(Pid::from_raw(*pid as i32), Signal::SIGSTOP);
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
            let _ = kill(Pid::from_raw(*pid as i32), Signal::SIGCONT);
        }
        drop(state);
        self.slot_free.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        self.state.lock().unwrap().paused
    }

    pub fn running(&self) -> usize {
        self.state.lock().unwrap().children.len()
    }
}
