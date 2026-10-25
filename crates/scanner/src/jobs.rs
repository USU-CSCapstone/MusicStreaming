//! What the background decodes, analysis and placeholders, make of a file they could not decode
//! (`design/scanning.md` §12):
//!
//! - **A verdict on the file**: a decoder's error on a file that opens, or a second failure of
//!   the file of a kind that may pass, such as a stall, with no success between. It is stored
//!   as an empty result, so the file is not decoded on every pass, and recorded as a scan
//!   problem.
//! - **A file that cannot be opened**, because it is gone, out of reach, not permitted, a
//!   symlink or not a regular file: never given a result. It is recorded as a problem and
//!   skipped for [`RETRY_AFTER`]. Once it decodes, its `Unreadable` or `PermissionDenied`
//!   problem is cleared.
//! - **Anything else**: an unavailable root, a decoder that cannot start or that the server
//!   killed as it stops, a first failure that may pass, or a store error. The pass ends and the
//!   job backs off, so an outage never strips results, and a failure of the store never counts
//!   against a file.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::discover::check_root;
use crate::library::LibraryId;
use crate::problems::{self, Problem};
use crate::store::{FileToDecode, ProblemLog, StoreError};
use jewelcase_core::Recover;

/// How long a file that cannot be opened is skipped before it is tried again.
pub const RETRY_AFTER: Duration = Duration::from_hours(1);

/// What one pass of a background job did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RunReport {
    /// Files decoded, with their results stored.
    pub done: usize,
    /// Files judged broken and given an empty result.
    pub failed: usize,
    /// Files that could not be opened, left to try again later. They do not count towards a
    /// pass's limit.
    pub skipped: usize,
    /// The pass ended early, and the job should wait before its next.
    pub backed_off: bool,
}

impl RunReport {
    /// The report of a pass that `error` from the store ended: logs the error and backs off.
    pub fn store_failed(error: StoreError) -> RunReport {
        tracing::warn!(%error, "store failed; backing off");
        RunReport { backed_off: true, ..RunReport::default() }
    }

    /// Whether the job should wait before its next pass: it backed off, or decoded and
    /// judged nothing.
    pub fn idle(&self) -> bool {
        self.backed_off || self.done + self.failed == 0
    }
}

/// How a decode failed.
#[derive(Debug)]
pub enum Failure {
    /// The file did not open.
    Open(io::Error),
    /// The decoder rejected the file's content.
    Content(Problem),
    /// This attempt failed, and the next may not: a stall, a decoder killed by a signal, or
    /// output that broke off.
    Attempt(Problem),
    /// No decoder could start, or the server stopped it.
    Host,
}

impl Failure {
    /// What an ffmpeg error decoding the file at `path` says.
    pub fn from_ffmpeg(path: &Path, error: &jewelcase_ffmpeg::Error) -> Failure {
        use jewelcase_ffmpeg::Error as F;
        match error {
            F::Spawn { .. } | F::Stopped => Failure::Host,
            F::Stalled | F::Io(_) | F::Killed { .. } => {
                Failure::Attempt(problems::from_ffmpeg(path, error))
            }
            F::Failed { .. } | F::Probe(_) | F::Image(_) | F::NoAudioStream => {
                Failure::Content(problems::from_ffmpeg(path, error))
            }
        }
    }
}

/// What the job does with the file it failed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Store an empty result: the problem is recorded.
    Verdict,
    /// Go on to the next file: this one waits, with its problem recorded.
    Skipped,
    /// End the pass and back off, leaving the file as it is.
    BackOff,
}

/// One job's memory of its failures, and of the files its passes have in hand, for one library.
/// Kept in memory, so a restart tries every file afresh.
pub struct Retries {
    retry_after: Duration,
    /// The files whose decode failed once in a way that may pass, until a decode of the file
    /// succeeds or the file is judged.
    struck: Mutex<HashSet<PathBuf>>,
    /// The files that could not be opened, each with when it may be tried again.
    waiting: Mutex<HashMap<PathBuf, Instant>>,
    /// The files that passes running at once have taken, so that no two decode the same file
    /// ([`Retries::take`]).
    in_hand: Mutex<HashSet<PathBuf>>,
}

/// A file a pass has taken. Dropping it lets other passes take the file.
struct Claim<'a> {
    retries: &'a Retries,
    path: PathBuf,
}

impl Drop for Claim<'_> {
    /// Gives the file back.
    fn drop(&mut self) {
        self.retries.in_hand().remove(&self.path);
    }
}

impl Retries {
    /// Skips a file that cannot be opened for `retry_after`.
    pub fn new(retry_after: Duration) -> Retries {
        Retries {
            retry_after,
            struck: Mutex::default(),
            waiting: Mutex::default(),
            in_hand: Mutex::default(),
        }
    }

    /// How many files to fetch for a pass that decodes up to `limit`, while `in_hand` files are
    /// held by other passes: room for those, and for the files still waiting and as many again
    /// found unopenable during the pass, so none of them crowds out the rest. Forgets the files
    /// whose wait is over.
    fn fetch(&self, limit: usize, in_hand: usize) -> usize {
        let mut waiting = self.waiting.lock().recover();
        let now = Instant::now();
        waiting.retain(|_, until| *until > now);
        limit.saturating_add(in_hand).saturating_add(waiting.len().saturating_mul(2))
    }

    /// The files passes have in hand. Its lock is held while the store is asked for files,
    /// which may panic; the set is whole whatever panicked, so a poisoned lock is taken as it
    /// is, and a [`Claim`] dropped while a thread unwinds never panics again.
    fn in_hand(&self) -> MutexGuard<'_, HashSet<PathBuf>> {
        self.in_hand.lock().recover()
    }

    /// Fetches files with `next`, given how many to fetch, and takes those that are neither
    /// waiting nor in another pass's hand, up to this pass's share of the fetch: what it is
    /// fetched for beyond the files in hand. Returns each taken file with its key and its
    /// [`Claim`]. The fetch and the taking hold the lock a [`Claim`] needs to give its file back,
    /// and a pass gives a decoded file back only after its result is stored, so a fetch never
    /// returns a file that another pass has decoded and not yet stored.
    fn take<K>(
        &self,
        limit: usize,
        next: impl FnOnce(usize) -> Result<Vec<(K, FileToDecode)>, StoreError>,
    ) -> Result<Vec<(K, FileToDecode, Claim<'_>)>, StoreError> {
        let mut in_hand = self.in_hand();
        let fetch = self.fetch(limit, in_hand.len());
        let share = fetch - in_hand.len();
        let files = next(fetch)?;
        // `take` stops the filter once the share is taken, so no file past it is put in hand.
        let free = files
            .into_iter()
            .filter(|(_, file)| !self.waiting(file) && in_hand.insert(file.path.clone()))
            .take(share);
        let claim = |(key, file): (K, FileToDecode)| {
            let path = file.path.clone();
            (key, file, Claim { retries: self, path })
        };
        Ok(free.map(claim).collect())
    }

    /// One pass of a job: `next` fetches files from the store, given how many to fetch, and the
    /// pass decodes them in order with `decode` until `limit` are decoded or judged, skipping
    /// those still waiting and those that a pass running at the same time has in hand. `keep`
    /// stores what was made of a file, or `None` for a verdict's empty result. A store error
    /// ends the pass, leaving the file it was on as it was, and is returned for
    /// [`RunReport::store_failed`].
    pub fn pass<K, T>(
        &self,
        store: &dyn ProblemLog,
        library: LibraryId,
        limit: usize,
        next: impl FnOnce(usize) -> Result<Vec<(K, FileToDecode)>, StoreError>,
        decode: impl Fn(&FileToDecode) -> Result<T, Failure>,
        keep: impl Fn(K, Option<T>) -> Result<(), StoreError>,
    ) -> Result<RunReport, StoreError> {
        let mut report = RunReport::default();
        // Each file's claim is dropped, giving the file back, once the loop is done with it.
        for (key, file, _claim) in self.take(limit, next)? {
            if report.done + report.failed >= limit {
                break;
            }
            match decode(&file) {
                Ok(result) => {
                    self.succeeded(store, library, &file)?;
                    keep(key, Some(result))?;
                    report.done += 1;
                }
                Err(failure) => match self.failed(store, library, &file, failure)? {
                    Outcome::Verdict => {
                        keep(key, None)?;
                        report.failed += 1;
                    }
                    Outcome::Skipped => report.skipped += 1,
                    Outcome::BackOff => {
                        report.backed_off = true;
                        break;
                    }
                },
            }
        }
        Ok(report)
    }

    /// Whether `file` is still waiting to be tried again.
    fn waiting(&self, file: &FileToDecode) -> bool {
        self.waiting.lock().recover().get(&file.path).is_some_and(|until| Instant::now() < *until)
    }

    /// Clears any problem saying `file` could not be opened ([`problems::UNOPENED`]), whoever
    /// recorded it, and notes that it decoded. It runs before the file's result is kept, so a
    /// store error leaves the file to decode again.
    fn succeeded(
        &self,
        store: &dyn ProblemLog,
        library: LibraryId,
        file: &FileToDecode,
    ) -> Result<(), StoreError> {
        store.clear_unopened(library, &file.path)?;
        self.struck.lock().recover().remove(&file.path);
        self.waiting.lock().recover().remove(&file.path);
        Ok(())
    }

    /// Judges a failed decode of `file` and records any problem. A file that cannot be opened
    /// waits only once its problem is recorded, so a store error leaves it as it was.
    fn failed(
        &self,
        store: &dyn ProblemLog,
        library: LibraryId,
        file: &FileToDecode,
        failure: Failure,
    ) -> Result<Outcome, StoreError> {
        let Some((outcome, problem)) = self.judge(file, failure) else {
            tracing::debug!(path = %file.path.display(), "decode failed; backing off");
            return Ok(Outcome::BackOff);
        };
        tracing::warn!(path = %file.path.display(), kind = ?problem.kind, "cannot decode: {}", problem.detail);
        store.record_problem(library, problem)?;
        let mut waiting = self.waiting.lock().recover();
        if outcome == Outcome::Skipped {
            waiting.insert(file.path.clone(), Instant::now() + self.retry_after);
        } else {
            waiting.remove(&file.path);
        }
        Ok(outcome)
    }

    /// What to do about `failure`, with the problem to record; `None` to back off.
    fn judge(&self, file: &FileToDecode, failure: Failure) -> Option<(Outcome, Problem)> {
        let error = match failure {
            Failure::Host => return None,
            // A decoder fails on a file that went away mid-decode just as on a broken one.
            Failure::Content(problem) => match file.open() {
                Ok(_) => return Some((Outcome::Verdict, problem)),
                Err(error) => error,
            },
            Failure::Open(error) => error,
            Failure::Attempt(problem) => {
                check_root(&file.root).ok()?;
                let mut struck = self.struck.lock().recover();
                if struck.insert(file.path.clone()) {
                    return None;
                }
                struck.remove(&file.path);
                return Some((Outcome::Verdict, problem));
            }
        };
        let problem = problems::from_io(&file.path, &error);
        // An unavailable root hides every file under it: nothing to record about this one.
        check_root(&file.root).ok()?;
        Some((Outcome::Skipped, problem))
    }
}

#[cfg(test)]
mod tests;
