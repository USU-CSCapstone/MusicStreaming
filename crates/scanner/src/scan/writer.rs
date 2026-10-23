//! The batch a scan fills, and the checkpoint before each directory, file and missing mark where
//! the batch is flushed and the scan stops if asked (`design/scanning.md` §7, §9).

use std::time::{Duration, Instant};

use super::{Cursor, Scan, ScanContext, ScanOptions, ScanProgress, ScanState};
use crate::clock::now_ms;
use crate::store::{Batch, StoreError};

/// Why a scan leaves its walk early.
pub(super) enum Halt {
    /// The queue asked it to stop, to cancel or to shut down. `checkpoint` does not flush the
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

/// The batch a scan fills, how many outcomes it flushes at, when it was last flushed, and the
/// cursor and progress the store holds, where the scan ends if a batch fails or it panics.
pub(super) struct Writer {
    pub batch: Batch,
    limit: usize,
    last_flush: Instant,
    persisted: Persisted,
}

impl Writer {
    /// A writer for `scan`, whose cursor and progress the store already holds. Its first batch
    /// flushes at the smallest size `options` allow, since nothing has yet measured how fast the
    /// store applies them.
    pub fn new(scan: &Scan, options: &ScanOptions) -> Writer {
        Writer {
            batch: Batch::default(),
            limit: options.min_batch.min(options.batch_size),
            last_flush: Instant::now(),
            persisted: (scan.cursor.clone(), scan.progress.clone()),
        }
    }

    /// Called before each directory, file and missing mark. Flushes the batch once it holds as
    /// many outcomes as it was sized for or has waited `batch_interval`, and then stops the scan if
    /// the queue has asked. Short of those it costs a few atomic loads and a clock read.
    pub fn checkpoint(&mut self, ctx: &ScanContext<'_>, scan: &mut Scan) -> Result<(), Halt> {
        if self.batch.size() >= self.limit
            || self.last_flush.elapsed() >= ctx.options.batch_interval
        {
            self.flush(ctx, scan)?;
        }
        if ctx.stopping() { Err(Halt::Stop) } else { Ok(()) }
    }

    /// Persists the batch, even an empty one, and the scan row in one transaction, the row's
    /// missing count raised by what the batch marked missing, and sizes the next batch from how
    /// long that took. On a store failure nothing is persisted, and the scan ends through
    /// [`Writer::fail`] and the error is [`Halt::Ended`].
    pub fn flush(&mut self, ctx: &ScanContext<'_>, scan: &mut Scan) -> Result<(), Halt> {
        let full = Batch { scan: Some(scan.clone()), ..std::mem::take(&mut self.batch) };
        let size = full.size();
        let started = Instant::now();
        let applied = ctx.store.apply(ctx.library.id, full);
        scan.progress.missing += applied.map_err(|e| self.fail(ctx, scan, &e))?;
        self.limit = fit(ctx.options, self.limit, size, started.elapsed());
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

    /// Ends `scan` at the cursor and progress the store holds and saves its row: suspended, or
    /// cancelled if a cancel was asked for, since a cancelled scan's work is not wanted.
    /// [`super::run_scan`] says what of a suspended scan's work is retried. A row that cannot be
    /// saved is logged and stays as the store last held it, queued or running, so the next start
    /// resumes the scan from there.
    pub fn suspend(&self, ctx: &ScanContext<'_>, scan: &mut Scan) {
        scan.cursor = self.persisted.0.clone();
        scan.progress = self.persisted.1.clone();
        scan.progress.current_path = None;
        scan.state = if ctx.cancelled() { ScanState::Cancelled } else { ScanState::Suspended };
        scan.finished_at = Some(now_ms());
        if let Err(error) = ctx.store.update_scan(scan) {
            tracing::error!(scan = scan.id, %error, "scan row not saved as it ended");
        }
    }
}

/// How many outcomes the next batch holds, after one of `size` applied in `took` while batches
/// held `limit`: as many as apply within `batch_time` at that rate, at least `min_batch` and at
/// most `batch_size`. A batch that writes no track, folder or problem says nothing about what
/// they cost, so `limit` stays. `took` counts any wait for the writer behind other writes, so a
/// busy writer makes batches smaller.
fn fit(options: &ScanOptions, limit: usize, size: usize, took: Duration) -> usize {
    if size == 0 {
        return limit;
    }
    // The float-to-integer `as` saturates: an infinite ratio, from a batch applied in no
    // measurable time, gives `i64::MAX`, past any `batch_size`.
    let fit = (options.batch_time.div_duration_f64(took) * size as f64) as i64;
    let fit = usize::try_from(fit.max(0)).unwrap_or(usize::MAX);
    fit.clamp(options.min_batch.min(options.batch_size), options.batch_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Batches sized to apply in about 50 ms, of 250 outcomes at most and `min_batch` at least.
    fn options(min_batch: usize) -> ScanOptions {
        ScanOptions {
            batch_time: Duration::from_millis(50),
            batch_size: 250,
            min_batch,
            ..Default::default()
        }
    }

    #[test]
    fn a_batch_is_sized_to_apply_within_the_time_at_the_last_ones_rate() {
        let ms = Duration::from_millis;
        // (min_batch, limit, size, took, next limit)
        let cases = [
            (1, 10, 10, ms(25), 20),
            (1, 10, 10, ms(100), 5),
            (1, 1, 1, ms(1000), 1),
            (1, 1, 1, Duration::ZERO, 250),
            (1, 250, 1000, ms(1), 250),
            // A store slower than the time allows still gets `min_batch`.
            (16, 16, 16, ms(1000), 16),
            (16, 16, 16, ms(25), 32),
            // A batch that wrote only cleared paths leaves the limit as it was.
            (16, 40, 0, ms(1000), 40),
            (1, 7, 0, Duration::ZERO, 7),
        ];
        for (min_batch, limit, size, took, next) in cases {
            let fitted = fit(&options(min_batch), limit, size, took);
            assert_eq!(fitted, next, "min_batch {min_batch}, {size} in {took:?}");
        }
    }

    #[test]
    fn min_batch_never_exceeds_batch_size() {
        let options = ScanOptions { batch_size: 8, ..options(16) };
        assert_eq!(fit(&options, 8, 8, Duration::from_secs(1)), 8);
    }
}
