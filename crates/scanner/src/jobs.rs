//! What the background decodes, analysis and placeholders, make of a file they could not decode
//! (`design/scanning.md` §12):
//!
//! - **A verdict on the file**: a decoder's error on a file that opens, a symlink or something
//!   other than a regular file refused, or a second failure in a row of a kind that may pass,
//!   such as a stall. It is stored as an empty result, so the file is not decoded on every pass,
//!   and recorded as a scan problem.
//! - **A file that cannot be opened**, because it is gone, out of reach or not permitted: never
//!   given a result. It is recorded as a problem and skipped for [`RETRY_AFTER`]. Once it
//!   decodes, its `Unreadable` or `PermissionDenied` problem is cleared.
//! - **Anything else**: an unavailable root, a decoder that cannot start, or a first failure that
//!   may pass. The pass ends and the worker backs off, so an outage never strips results.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::confine::is_refused;
use crate::discover::check_root;
use crate::problems;
use crate::store::{Batch, FileToDecode, Store};
use crate::types::{LibraryId, Problem};

/// How long a file that cannot be opened is skipped before it is tried again.
pub const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

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
    /// The pass ended early, and the worker should wait before the next.
    pub backed_off: bool,
}

impl RunReport {
    /// Whether the worker should sleep before the next pass: it backed off, or decoded and
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
    /// No decoder could start.
    Host,
}

impl Failure {
    /// What an ffmpeg error decoding the file at `path` says.
    pub fn from_ffmpeg(path: &Path, error: &jewelcase_ffmpeg::Error) -> Failure {
        use jewelcase_ffmpeg::Error as F;
        match error {
            F::Spawn { .. } => Failure::Host,
            F::Stalled(_) | F::Io(_) | F::Killed { .. } => {
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

/// One job's memory of its failures, for one library. Kept in memory, so a restart tries every
/// file afresh.
pub struct Retries {
    retry_after: Duration,
    /// The file whose decode last failed in a way that may pass, until a decode succeeds or the
    /// file is judged.
    last_failed: Mutex<Option<PathBuf>>,
    /// The files that could not be opened, each with when it may be tried again.
    waiting: Mutex<HashMap<PathBuf, Instant>>,
}

impl Retries {
    /// Skips a file that cannot be opened for `retry_after`.
    pub fn new(retry_after: Duration) -> Retries {
        Retries { retry_after, last_failed: Mutex::new(None), waiting: Mutex::default() }
    }

    /// How many files to fetch for a pass that decodes up to `limit`: room for the files still
    /// waiting, and as many again found unopenable during the pass, so neither crowds out the
    /// rest. Forgets the files whose wait is over.
    pub fn fetch(&self, limit: usize) -> usize {
        let mut waiting = self.waiting.lock().unwrap();
        let now = Instant::now();
        waiting.retain(|_, until| *until > now);
        limit.saturating_add(waiting.len().saturating_mul(2))
    }

    /// One pass of a job over `queue`, the files the store handed it, in order: decodes them with
    /// `decode` until `limit` are decoded or judged, skipping those still waiting. `keep` stores
    /// what was made of a file, or `None` for a verdict's empty result.
    pub fn pass<K, T>(
        &self,
        store: &dyn Store,
        library: &LibraryId,
        queue: Vec<(K, FileToDecode)>,
        limit: usize,
        decode: impl Fn(&FileToDecode) -> Result<T, Failure>,
        keep: impl Fn(K, Option<T>),
    ) -> RunReport {
        let mut report = RunReport::default();
        for (key, file) in queue {
            if report.done + report.failed >= limit {
                break;
            }
            if self.waiting(&file) {
                continue;
            }
            match decode(&file) {
                Ok(result) => {
                    keep(key, Some(result));
                    self.succeeded(store, library, &file);
                    report.done += 1;
                }
                Err(failure) => match self.failed(store, library, &file, failure) {
                    Outcome::Verdict => {
                        keep(key, None);
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
        report
    }

    /// Whether `file` is still waiting to be tried again.
    fn waiting(&self, file: &FileToDecode) -> bool {
        self.waiting.lock().unwrap().get(&file.path).is_some_and(|until| Instant::now() < *until)
    }

    /// Notes that `file` decoded, and clears any problem saying it could not be opened
    /// ([`problems::UNOPENED`]), whoever recorded it. A store error is logged and ignored.
    fn succeeded(&self, store: &dyn Store, library: &LibraryId, file: &FileToDecode) {
        *self.last_failed.lock().unwrap() = None;
        self.waiting.lock().unwrap().remove(&file.path);
        let batch = Batch { opened: vec![file.path.clone()], ..Batch::default() };
        if let Err(error) = store.apply(library, batch) {
            tracing::warn!(path = %file.path.display(), %error, "cannot clear a problem");
        }
    }

    /// Judges a failed decode of `file` and records any problem. A problem the store cannot
    /// record leaves the file as it is, and the worker backs off.
    fn failed(
        &self,
        store: &dyn Store,
        library: &LibraryId,
        file: &FileToDecode,
        failure: Failure,
    ) -> Outcome {
        let Some((outcome, problem)) = self.judge(file, failure) else {
            tracing::debug!(path = %file.path.display(), "decode failed; backing off");
            return Outcome::BackOff;
        };
        tracing::warn!(path = %file.path.display(), kind = ?problem.kind, "cannot decode: {}", problem.detail);
        match store.apply(library, Batch { problems: vec![problem], ..Batch::default() }) {
            Ok(()) => outcome,
            // The store has logged why.
            Err(_) => Outcome::BackOff,
        }
    }

    /// What to do about `failure`, with the problem to record; `None` to back off.
    fn judge(&self, file: &FileToDecode, failure: Failure) -> Option<(Outcome, Problem)> {
        let error = match failure {
            Failure::Host => return None,
            // A decoder fails on a file that went away mid-decode just as on a broken one.
            Failure::Content(problem) => match file.open() {
                Ok(_) => return Some(self.verdict(file, problem)),
                Err(error) => error,
            },
            Failure::Open(error) => error,
            Failure::Attempt(problem) => {
                check_root(&file.root).ok()?;
                let mut last = self.last_failed.lock().unwrap();
                if last.as_ref() != Some(&file.path) {
                    *last = Some(file.path.clone());
                    return None;
                }
                *last = None;
                return Some(self.verdict(file, problem));
            }
        };
        let problem = problems::from_io(&file.path, &error);
        if is_refused(&error) {
            return Some(self.verdict(file, problem));
        }
        // An unavailable root hides every file under it: nothing to record about this one.
        check_root(&file.root).ok()?;
        self.waiting.lock().unwrap().insert(file.path.clone(), Instant::now() + self.retry_after);
        Some((Outcome::Skipped, problem))
    }

    /// A verdict on `file`, which then no longer waits.
    fn verdict(&self, file: &FileToDecode, problem: Problem) -> (Outcome, Problem) {
        self.waiting.lock().unwrap().remove(&file.path);
        (Outcome::Verdict, problem)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::SystemTime;

    use super::*;
    use crate::store::MemoryStore;
    use crate::types::ProblemKind;

    /// A root holding the files `a` and `b`, and a store to record problems in.
    fn setup() -> (tempfile::TempDir, MemoryStore, [FileToDecode; 2]) {
        let tmp = tempfile::tempdir().unwrap();
        let file = |name: &str| {
            fs::write(tmp.path().join(name), b"x").unwrap();
            FileToDecode { root: tmp.path().to_path_buf(), path: tmp.path().join(name) }
        };
        let files = [file("a"), file("b")];
        (tmp, MemoryStore::new(), files)
    }

    /// A stall decoding `file`: a failure that may pass.
    fn stall(file: &FileToDecode) -> Failure {
        Failure::Attempt(Problem {
            path: file.path.clone(),
            kind: ProblemKind::Stalled,
            detail: "stalled".into(),
            seen_at: SystemTime::now(),
        })
    }

    #[test]
    fn a_second_failure_in_a_row_is_a_verdict() {
        let (_tmp, store, [a, b]) = setup();
        let retries = Retries::new(RETRY_AFTER);
        let lib = "1".to_owned();
        assert_eq!(retries.failed(&store, &lib, &a, stall(&a)), Outcome::BackOff);
        assert!(store.problems(&lib).is_empty());
        assert_eq!(retries.failed(&store, &lib, &a, stall(&a)), Outcome::Verdict);
        assert_eq!(store.problems(&lib).len(), 1);
        // Another file failing in between makes it a first failure again.
        assert_eq!(retries.failed(&store, &lib, &b, stall(&b)), Outcome::BackOff);
        assert_eq!(retries.failed(&store, &lib, &a, stall(&a)), Outcome::BackOff);
    }

    #[test]
    fn a_success_resets_the_count() {
        let (_tmp, store, [a, _]) = setup();
        let retries = Retries::new(RETRY_AFTER);
        let lib = "1".to_owned();
        assert_eq!(retries.failed(&store, &lib, &a, stall(&a)), Outcome::BackOff);
        retries.succeeded(&store, &lib, &a);
        assert_eq!(retries.failed(&store, &lib, &a, stall(&a)), Outcome::BackOff);
        assert!(store.problems(&lib).is_empty());
    }

    #[test]
    fn a_failure_under_an_unavailable_root_backs_off_unrecorded() {
        let (tmp, store, [a, _]) = setup();
        let retries = Retries::new(RETRY_AFTER);
        let lib = "1".to_owned();
        fs::rename(tmp.path(), tmp.path().with_extension("away")).unwrap();
        for _ in 0..3 {
            let gone = Failure::Open(io::ErrorKind::NotFound.into());
            assert_eq!(retries.failed(&store, &lib, &a, gone), Outcome::BackOff);
            assert_eq!(retries.failed(&store, &lib, &a, stall(&a)), Outcome::BackOff);
        }
        assert!(store.problems(&lib).is_empty());
        assert!(!retries.waiting(&a));
        fs::rename(tmp.path().with_extension("away"), tmp.path()).unwrap();
    }

    #[test]
    fn a_decoder_error_on_a_file_that_went_away_is_no_verdict() {
        let (_tmp, store, [a, _]) = setup();
        let retries = Retries::new(RETRY_AFTER);
        let lib = "1".to_owned();
        fs::remove_file(&a.path).unwrap();
        let rejected = jewelcase_ffmpeg::Error::Failed { status: Some(1), stderr: String::new() };
        let failure = Failure::from_ffmpeg(&a.path, &rejected);
        assert_eq!(retries.failed(&store, &lib, &a, failure), Outcome::Skipped);
        assert!(retries.waiting(&a));
        assert_eq!(store.problems(&lib)[0].kind, ProblemKind::Unreadable);
    }

    #[test]
    fn files_that_cannot_be_opened_do_not_count_towards_the_limit() {
        let (tmp, store, [a, b]) = setup();
        let retries = Retries::new(RETRY_AFTER);
        let lib = "1".to_owned();
        let gone = FileToDecode { root: tmp.path().to_path_buf(), path: tmp.path().join("gone") };
        let queue = vec![(1, gone.clone()), (2, a.clone()), (3, b)];
        fs::remove_file(&a.path).unwrap();
        let decode = |file: &FileToDecode| file.open().map_err(Failure::Open);
        let report = retries.pass(&store, &lib, queue, 1, decode, |_, _| ());
        assert_eq!(report, RunReport { done: 1, skipped: 2, ..RunReport::default() });
        // Two files wait, with room for as many more found unopenable.
        assert_eq!(retries.fetch(1), 5);
        let queue = vec![(1, gone), (2, a)];
        assert_eq!(retries.pass(&store, &lib, queue, 1, decode, |_, _| ()), RunReport::default());
    }

    #[test]
    fn a_file_whose_wait_is_over_is_forgotten() {
        let (_tmp, store, [a, _]) = setup();
        let retries = Retries::new(Duration::ZERO);
        let lib = "1".to_owned();
        fs::remove_file(&a.path).unwrap();
        let gone = Failure::Open(io::ErrorKind::NotFound.into());
        assert_eq!(retries.failed(&store, &lib, &a, gone), Outcome::Skipped);
        assert_eq!(retries.fetch(1), 1);
        assert!(retries.waiting.lock().unwrap().is_empty());
    }

    #[test]
    fn a_pass_that_backed_off_is_idle_whatever_it_did() {
        assert!(RunReport { done: 5, backed_off: true, ..RunReport::default() }.idle());
        assert!(RunReport { skipped: 3, ..RunReport::default() }.idle(), "only skipped");
        assert!(!RunReport { done: 1, skipped: 3, ..RunReport::default() }.idle());
        assert!(RunReport::default().idle());
    }

    #[test]
    fn a_decoder_killed_by_a_signal_may_pass() {
        let path = Path::new("/music/a.flac");
        let killed = jewelcase_ffmpeg::Error::Killed { signal: 9, stderr: String::new() };
        assert!(matches!(Failure::from_ffmpeg(path, &killed), Failure::Attempt(_)));
        let rejected = jewelcase_ffmpeg::Error::Failed { status: Some(1), stderr: String::new() };
        assert!(matches!(Failure::from_ffmpeg(path, &rejected), Failure::Content(_)));
    }
}
