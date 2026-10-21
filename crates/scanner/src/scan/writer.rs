//! The batch a scan fills, and the checkpoint before each directory and file where the batch is
//! flushed, the scan parks for the governor, and the scan stops if asked (`design/scanning.md`
//! §7, §9, §10).

use std::time::{Instant, SystemTime};

use super::ScanContext;
use crate::store::Batch;
use crate::types::*;

/// Why a scan leaves its walk early.
pub(super) enum Halt {
    /// The queue asked it to stop, to cancel or to shut down. `checkpoint` does not flush the
    /// batch in hand; the caller does.
    Stop,
    /// The store failed a batch, and the scan has ended at its last persisted cursor.
    Ended,
}

/// The cursor and progress the store holds for a scan.
type Persisted = (Option<Cursor>, ScanProgress);

/// The batch a scan fills, when it was last flushed, and the cursor and progress the store holds,
/// where the scan ends if a batch fails or it panics.
pub(super) struct Writer {
    pub batch: Batch,
    last_flush: Instant,
    persisted: Persisted,
}

impl Writer {
    /// A writer for `scan`, whose cursor and progress the store already holds.
    pub fn new(scan: &Scan) -> Writer {
        Writer {
            batch: Batch::new(scan.id),
            last_flush: Instant::now(),
            persisted: (scan.cursor.clone(), scan.progress.clone()),
        }
    }

    /// Called before each directory and each file. Parks while the governor is paused, flushes the
    /// batch once it holds `batch_size` outcomes or has waited `batch_interval`, and then stops
    /// the scan if the queue has asked. Short of those it costs an uncontended lock, a few atomic
    /// loads and a clock read.
    pub fn checkpoint(&mut self, ctx: &ScanContext<'_>, scan: &mut Scan) -> Result<(), Halt> {
        ctx.governor.wait_if_paused();
        if self.batch.len() >= ctx.options.batch_size
            || self.last_flush.elapsed() >= ctx.options.batch_interval
        {
            self.flush(ctx, scan)?;
        }
        if ctx.stopping() { Err(Halt::Stop) } else { Ok(()) }
    }

    /// Persists the batch and the scan row. On a store failure the scan ends at its last persisted
    /// cursor through [`Writer::suspend`] rather than reporting success over lost work
    /// (`requirements/scanning.md` §10: problems are honest), and the error is [`Halt::Ended`].
    pub fn flush(&mut self, ctx: &ScanContext<'_>, scan: &mut Scan) -> Result<(), Halt> {
        if !self.batch.is_empty() {
            let full = std::mem::replace(&mut self.batch, Batch::new(scan.id));
            if let Err(e) = ctx.store.apply(&ctx.library.id, full) {
                tracing::error!(scan = scan.id, error = %e, "batch not persisted; suspending scan");
                self.suspend(ctx, scan);
                return Err(Halt::Ended);
            }
        }
        self.persisted = (scan.cursor.clone(), scan.progress.clone());
        self.last_flush = Instant::now();
        ctx.store.update_scan(scan);
        Ok(())
    }

    /// Ends `scan` at the cursor and progress the store holds and saves its row: suspended, so a
    /// resume retries what was not persisted, or cancelled if a cancel was asked for, since a
    /// cancelled scan never resumes.
    pub fn suspend(&self, ctx: &ScanContext<'_>, scan: &mut Scan) {
        scan.cursor = self.persisted.0.clone();
        scan.progress = self.persisted.1.clone();
        scan.progress.current_path = None;
        scan.state = if ctx.cancelled() { ScanState::Cancelled } else { ScanState::Suspended };
        scan.finished_at = Some(SystemTime::now());
        ctx.store.update_scan(scan);
    }
}
