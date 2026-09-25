//! Yielding to listeners (`design/scanning.md` §10).
//!
//! The governor only ever slows background work. Indexing threads park at
//! directory boundaries; ffmpeg children are stopped with a signal and
//! continue where they were.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use jewelcase_ffmpeg::Registry;

pub struct Governor {
    paused: AtomicBool,
    lock: Mutex<()>,
    cv: Condvar,
    ffmpeg: Mutex<Option<Arc<Registry>>>,
}

impl Default for Governor {
    fn default() -> Self {
        Governor::new()
    }
}

impl Governor {
    pub fn new() -> Governor {
        Governor {
            paused: AtomicBool::new(false),
            lock: Mutex::new(()),
            cv: Condvar::new(),
            ffmpeg: Mutex::new(None),
        }
    }

    /// Attach the ffmpeg pool so pausing stops decodes too.
    pub fn attach_ffmpeg(&self, registry: Arc<Registry>) {
        *self.ffmpeg.lock().unwrap() = Some(registry);
    }

    /// Pause all background work. Idempotent.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        if let Some(reg) = self.ffmpeg.lock().unwrap().as_ref() {
            reg.pause_all();
        }
    }

    /// Resume. Idempotent.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        if let Some(reg) = self.ffmpeg.lock().unwrap().as_ref() {
            reg.resume_all();
        }
        let _guard = self.lock.lock().unwrap();
        self.cv.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// Block the calling background thread while paused. Wakes periodically
    /// so a missed notification cannot strand a scan.
    pub fn wait_if_paused(&self) {
        let mut guard = self.lock.lock().unwrap();
        while self.paused.load(Ordering::SeqCst) {
            let (g, _) = self
                .cv
                .wait_timeout(guard, Duration::from_millis(250))
                .unwrap();
            guard = g;
        }
    }
}
