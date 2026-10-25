//! The batch a scan fills, and the checkpoint before each directory, file and missing mark where
//! the batch is flushed and the scan stops if asked (`design/scanning.md` §7, §9).

use std::time::Instant;

use super::{Cursor, Scan, ScanContext, ScanProgress, ScanState};
use crate::clock::now_ms;
use crate::store::{Batch, StoreError};

/// Why a scan leaves its walk early.
pub(super) enum Halt {
    /// The queue asked it to stop for a shutdown. `checkpoint` does not flush the
    /// batch in hand; the caller does.
    Stop,
    /// The store failed, and the scan has ended at its last persisted cursor.
    Ended,
    /// The scope's root stopped answering. The caller flushes the batch in hand and leaves the
    /// rest of the scope for a retry.
    Unanswered,
}

/// The cursor and progress the store holds for a scan.
type Persisted = (Option<Cursor>, ScanProgress);

/// The batch a scan fills, when it was last flushed, and the cursor and progress the store
/// holds, where the scan ends if a batch fails or it panics.
pub(super) struct Writer {
    pub batch: Batch,
    last_flush: Instant,
    persisted: Persisted,
}

impl Writer {
    /// A writer for `scan`, whose cursor and progress the store already holds.
    pub fn new(scan: &Scan) -> Writer {
        Writer {
            batch: Batch::default(),
            last_flush: Instant::now(),
            persisted: (scan.cursor.clone(), scan.progress.clone()),
        }
    }

    /// Called before each directory, file and missing mark. Flushes the batch once it holds
    /// `batch_size` outcomes or has waited `batch_interval`, and then stops the scan if
    /// the queue has asked. Short of those it costs a few atomic loads and a clock read.
    pub fn checkpoint(&mut self, ctx: &ScanContext<'_>, scan: &mut Scan) -> Result<(), Halt> {
        if self.batch.size() >= ctx.options.batch_size
            || self.last_flush.elapsed() >= ctx.options.batch_interval
        {
            self.flush(ctx, scan)?;
        }
        if ctx.stopping() { Err(Halt::Stop) } else { Ok(()) }
    }

    /// Persists the batch, even an empty one, and the scan row in one transaction, the row's
    /// missing count raised by what the batch marked missing. On a store failure nothing is
    /// persisted, and the scan ends through [`Writer::fail`] and the error is [`Halt::Ended`].
    pub fn flush(&mut self, ctx: &ScanContext<'_>, scan: &mut Scan) -> Result<(), Halt> {
        let full = Batch { scan: Some(scan.clone()), ..std::mem::take(&mut self.batch) };
        let applied = ctx.store.apply(ctx.library.id, full);
        scan.progress.missing += applied.map_err(|e| self.fail(ctx, scan, &e))?;
        self.persisted = (scan.cursor.clone(), scan.progress.clone());
        self.last_flush = Instant::now();
        Ok(())
    }

    /// Saves the scan row. On a store failure the scan ends through [`Writer::fail`] and the
    /// error is [`Halt::Ended`].
    pub fn save(&self, ctx: &ScanContext<'_>, scan: &mut Scan) -> Result<(), Halt> {
        ctx.store.update_scan(scan).map_err(|e| self.fail(ctx, scan, &e))
    }

    /// Ends `scan` after the store failed with `error`: logs it and ends the scan at its last
    /// persisted cursor through [`Writer::suspend`] rather than reporting success over lost work
    /// (`requirements/scanning.md` §10: problems are honest). Returns [`Halt::Ended`] for the
    /// caller to stop with.
    pub fn fail(&self, ctx: &ScanContext<'_>, scan: &mut Scan, error: &StoreError) -> Halt {
        tracing::error!(scan = scan.id, %error, "store failed; suspending scan");
        self.suspend(ctx, scan);
        Halt::Ended
    }

    /// Ends `scan` at the cursor and progress the store holds and saves its row, suspended.
    /// [`super::run_scan`] says what of a suspended scan's work is retried. A row that cannot be
    /// saved is logged and stays as the store last held it, queued or running, so the next start
    /// resumes the scan from there.
    pub fn suspend(&self, ctx: &ScanContext<'_>, scan: &mut Scan) {
        scan.cursor = self.persisted.0.clone();
        scan.progress = self.persisted.1.clone();
        scan.progress.current_path = None;
        scan.state = ScanState::Suspended;
        scan.finished_at = Some(now_ms());
        if let Err(error) = ctx.store.update_scan(scan) {
            tracing::error!(scan = scan.id, %error, "scan row not saved as it ended");
        }
    }
}
