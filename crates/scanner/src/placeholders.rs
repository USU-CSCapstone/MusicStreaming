//! Image placeholders: a ThumbHash of every image, from which a client draws a blurred,
//! correctly coloured stand-in before the image itself arrives, or offline without it
//! (`requirements/offline.md` §1.1). About 25 bytes each.
//!
//! Filled in the background after a scan finds an image, the same way analysis follows it, and
//! at the same low priority.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use jewelcase_ffmpeg::Ffmpeg;

use crate::jobs::{Failure, RETRY_AFTER, Retries, RunReport};
use crate::store::{FileToDecode, Store};
use crate::types::LibraryId;

/// ThumbHash encodes at most 100 pixels a side, and gains nothing from more.
const MAX_SIDE: u32 = 100;

/// Fills placeholders for one library.
pub struct Placeholders {
    ffmpeg: Ffmpeg,
    store: Arc<dyn Store>,
    retries: Retries,
}

impl Placeholders {
    /// Makes placeholders with `ffmpeg` and stores them in `store`.
    pub fn new(ffmpeg: Ffmpeg, store: Arc<dyn Store>) -> Placeholders {
        Placeholders { ffmpeg, store, retries: Retries::new(RETRY_AFTER) }
    }

    /// Skips an image whose file cannot be opened for `retry_after`, rather than an hour.
    pub fn with_retry_after(mut self, retry_after: Duration) -> Placeholders {
        self.retries = Retries::new(retry_after);
        self
    }

    /// The placeholder for the image at `path`, or the picture attached to an audio file.
    pub fn placeholder(&self, path: &Path) -> Result<Vec<u8>, jewelcase_ffmpeg::Error> {
        let image = self.ffmpeg.decode_image(path, MAX_SIDE)?;
        Ok(thumbhash::rgba_to_thumb_hash(image.width, image.height, &image.pixels))
    }

    /// Fills up to `limit` placeholders, skipping images whose files are waiting to be tried
    /// again. A failure is handled as [`crate::jobs`] describes; a verdict is stored empty, and
    /// new content is a new image, which gets a new try.
    pub fn run_once(&self, library: LibraryId, limit: usize) -> RunReport {
        let store = self.store.as_ref();
        let next = |fetch| store.next_without_placeholder(library, fetch);
        let decode = |file: &FileToDecode| {
            let path = file.open().map_err(Failure::Open)?;
            self.placeholder(&path).map_err(|error| Failure::from_ffmpeg(&file.path, &error))
        };
        let keep = |hash: Vec<u8>, placeholder: Option<Vec<u8>>| {
            store.store_placeholder(library, &hash, placeholder.unwrap_or_default())
        };
        let pass = self.retries.pass(store, library, limit, next, decode, keep);
        pass.unwrap_or_else(RunReport::store_failed)
    }

    /// Runs in the background, at low priority, until stopped, sleeping when idle or backing off.
    pub fn start(self: Arc<Self>, library: LibraryId, idle_sleep: Duration) -> PlaceholderWorker {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::Builder::new()
            .name("scanner-placeholders".into())
            .spawn(move || {
                jewelcase_ffmpeg::lower_thread_priority();
                while !stopping.load(Ordering::SeqCst) {
                    if self.run_once(library, 32).idle() {
                        // Woken early by `stop`.
                        std::thread::park_timeout(idle_sleep);
                    }
                }
            })
            .expect("spawn placeholder thread");
        PlaceholderWorker { stop, thread: Some(thread) }
    }
}

pub struct PlaceholderWorker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PlaceholderWorker {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}
