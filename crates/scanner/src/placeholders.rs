//! Image placeholders: a ThumbHash of every image, from which a client draws a blurred,
//! correctly coloured stand-in before the image itself arrives, or offline without it
//! (`requirements/offline.md` §1.1). About 25 bytes each.
//!
//! Filled in the background after a scan finds an image, the same way analysis follows it, and
//! at the same low priority.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use jewelcase_ffmpeg::Ffmpeg;

use crate::jobs::{Failure, RETRY_AFTER, Retries, RunReport};
use crate::library::LibraryId;
use crate::pool::Job;
use crate::store::{FileToDecode, PlaceholderStore};

/// ThumbHash encodes at most 100 pixels a side, and gains nothing from more.
const MAX_SIDE: u32 = 100;

/// How long the background job waits after a pass that backed off or found nothing to do:
/// briefly, so a placeholder follows soon after a scan finds its image, since clients draw it at
/// once.
const IDLE: Duration = Duration::from_secs(5);

/// Fills placeholders for one library.
pub struct Placeholders {
    ffmpeg: Ffmpeg,
    store: Arc<dyn PlaceholderStore>,
    retries: Retries,
}

impl Placeholders {
    /// Makes placeholders with `ffmpeg` and stores them in `store`.
    pub fn new(ffmpeg: Ffmpeg, store: Arc<dyn PlaceholderStore>) -> Placeholders {
        Placeholders { ffmpeg, store, retries: Retries::new(RETRY_AFTER) }
    }

    /// Skips an image whose file cannot be opened for `retry_after`, rather than an hour.
    #[cfg(feature = "testing")]
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
    /// again. A failure is handled as `jobs` describes; a verdict is stored empty, and
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

    /// The background job that fills `library`'s placeholders in the pool, waiting [`IDLE`]
    /// when idle or backing off.
    pub fn job(self: Arc<Self>, library: LibraryId) -> Job {
        Job::new(library, IDLE, move || self.run_once(library, 32))
    }
}
