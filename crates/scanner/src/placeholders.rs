//! Image placeholders: a ThumbHash of every image, from which a client draws a blurred,
//! correctly coloured stand-in before the image itself arrives, or offline without it
//! (`requirements/offline.md` §1.1). About 25 bytes each.
//!
//! Filled in the background after a scan finds an image, the same way analysis follows it.
//! Decodes share the ffmpeg pool with analysis, so the governor pauses both.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use jewelcase_ffmpeg::Ffmpeg;

use crate::store::Store;
use crate::types::LibraryId;

/// ThumbHash encodes at most 100 pixels a side, and gains nothing from more.
const MAX_SIDE: u32 = 100;

/// Fills placeholders for one library.
pub struct Placeholders {
    ffmpeg: Ffmpeg,
    store: Arc<dyn Store>,
}

impl Placeholders {
    pub fn new(ffmpeg: Ffmpeg, store: Arc<dyn Store>) -> Placeholders {
        Placeholders { ffmpeg, store }
    }

    /// The placeholder for the image at `path`, or the picture attached to an audio file.
    pub fn placeholder(&self, path: &Path) -> Result<Vec<u8>, jewelcase_ffmpeg::Error> {
        let image = self.ffmpeg.decode_image(path, MAX_SIDE)?;
        Ok(thumbhash::rgba_to_thumb_hash(
            image.width,
            image.height,
            &image.pixels,
        ))
    }

    /// Fills up to `limit` placeholders, and returns how many images it tried; zero means none
    /// are left.
    pub fn run_once(&self, library: &LibraryId, limit: usize) -> usize {
        let images = self.store.next_without_placeholder(library, limit);
        for (hash, path) in &images {
            let placeholder = self.placeholder(path).unwrap_or_else(|error| {
                // Stored empty, so a broken image is not decoded on every pass. New content
                // is a new image, which gets a new try.
                tracing::warn!(path = %path.display(), %error, "cannot make an image placeholder");
                Vec::new()
            });
            self.store.store_placeholder(library, hash, placeholder);
        }
        images.len()
    }

    /// Runs in the background until stopped, sleeping when idle.
    pub fn start(self: Arc<Self>, library: LibraryId, idle_sleep: Duration) -> PlaceholderWorker {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::Builder::new()
            .name("scanner-placeholders".into())
            .spawn(move || {
                while !stopping.load(Ordering::SeqCst) {
                    if self.run_once(&library, 32) == 0 {
                        // Woken early by `stop`.
                        std::thread::park_timeout(idle_sleep);
                    }
                }
            })
            .expect("spawn placeholder thread");
        PlaceholderWorker {
            stop,
            thread: Some(thread),
        }
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
