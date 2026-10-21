//! The pipeline over one scan (`design/scanning.md` §2, §7, §8).

use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::clock::now_ms;
use crate::confine::without_links;
use crate::discover::{
    Directory, RootUnavailable, Walk, WalkItem, check_root, compile_excludes, excluded_scope, gone,
};
use crate::library::LibraryConfig;
use crate::problems;
use crate::scope::Scope;
use crate::sidecar::SidecarResolver;
use crate::store::{Store, StoreError};
use file::{FileOutcome, process_file};
use writer::{Halt, Writer};

mod file;
mod row;
mod writer;

pub use row::{Cursor, Scan, ScanId, ScanProgress, ScanState, Trigger};

/// When a scan flushes its batch. The scanner always runs with the defaults, the design's
/// starting points; tests choose others to flush at the points they check.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Flush a batch at this many outcomes...
    pub batch_size: usize,
    /// ...or after this long, whichever comes first, so progress is visible
    /// and content appears within seconds.
    pub batch_interval: Duration,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions { batch_size: 250, batch_interval: Duration::from_secs(2) }
    }
}

/// What a scan needs from its surroundings.
pub struct ScanContext<'a> {
    pub store: &'a dyn Store,
    pub library: &'a LibraryConfig,
    /// Set by the queue to stop the scan after the file in hand and end it cancelled.
    pub cancel: &'a AtomicBool,
    /// Set by the queue to stop the scan at the same point but leave it running at its last
    /// flushed cursor, so the next start resumes it.
    pub shutdown: &'a AtomicBool,
    pub options: &'a ScanOptions,
}

impl ScanContext<'_> {
    /// Whether the queue has asked the scan to stop, to cancel it or to shut down.
    fn stopping(&self) -> bool {
        self.cancelled() || self.shutdown.load(Ordering::SeqCst)
    }

    /// Whether the queue has asked the scan to stop and end cancelled.
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// What a suspended scan left undone, by why, for a new scan to retry whole.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Undone {
    /// Each scope whose root did not answer, before its walk or before its reconcile.
    pub unanswered: Vec<Scope>,
    /// After a store failure, each scope from the last persisted cursor on.
    pub failed: Vec<Scope>,
}

/// Run `scan` to completion, cancellation, or suspension, or until a shutdown, which leaves it
/// running at its last flushed cursor for the next start to resume. A panic ends it at that
/// cursor through [`Writer::suspend`], so the caller carries on. The scan row is updated on every
/// flush; `scan` is left in the state the row holds, unless the store could not save it.
///
/// Returns what a suspended scan left undone, and nothing for any other end. That leaves out what
/// a panic or an exclude pattern that does not compile left unwalked, since a retry would panic
/// or suspend again, but not a root found not answering before a panic.
pub fn run_scan(ctx: &ScanContext<'_>, scan: &mut Scan) -> Undone {
    let mut writer = Writer::new(scan);
    let mut unanswered = Vec::new();
    // Asserted unwind safe because a panic can leave `scan`'s cursor and progress ahead of the
    // store, and `suspend` replaces both from what the writer last persisted, which changes only
    // as a whole, after a flush, and sets the state itself; a batch left half filled is dropped.
    // `unanswered` only grows, so a panic leaves it naming the roots that did not answer so far.
    // Everything else the scan touches is shared through `ctx` and keeps its own guarantees: the
    // store commits a batch whole or not at all.
    let walked = panic::catch_unwind(AssertUnwindSafe(|| {
        scan_scopes(ctx, scan, &mut writer, &mut unanswered)
    }));
    let failed = match walked {
        // Completed, stopped, or suspended with the roots that did not answer in `unanswered`.
        Ok(Ok(())) => Vec::new(),
        // Ended by a store failure that `Writer::fail` has handled.
        Ok(Err(_)) => from_cursor(scan).map(|(_, scope)| scope).collect(),
        Err(_) => {
            tracing::error!(
                scan = scan.id,
                "scan panicked; ending it at its last flushed cursor, without a retry of what it \
                 left, which would panic again"
            );
            writer.suspend(ctx, scan);
            Vec::new()
        }
    };
    if scan.state != ScanState::Suspended {
        return Undone::default();
    }
    Undone { unanswered, failed }
}

/// The scopes of `scan` from the one its cursor is in on, or all of them when it has no cursor,
/// each with its index. The iterator owns a copy of them, so `scan` can change as it is walked.
fn from_cursor(scan: &Scan) -> impl Iterator<Item = (i64, Scope)> + use<> {
    let from = scan.cursor.as_ref().map_or(0, |c| c.scope_index);
    (0..).zip(scan.scopes.clone()).skip_while(move |(i, _)| *i < from)
}

/// [`run_scan`] without the panic guard, flushing through `writer` and adding each scope whose
/// root does not answer to `unanswered`. Fails with [`Halt::Ended`] once a store failure has
/// ended the scan through [`Writer::fail`].
fn scan_scopes(
    ctx: &ScanContext<'_>,
    scan: &mut Scan,
    writer: &mut Writer,
    unanswered: &mut Vec<Scope>,
) -> Result<(), Halt> {
    // Leaving out a pattern that does not compile would index what the admin meant to exclude.
    let excludes = match compile_excludes(&ctx.library.excludes) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(scan = scan.id, error = %e, "invalid exclude pattern; suspending scan");
            writer.suspend(ctx, scan);
            return Ok(());
        }
    };

    scan.state = ScanState::Running;
    if scan.started_at.is_none() {
        scan.started_at = Some(now_ms());
    }
    writer.save(ctx, scan)?;

    // Folders the walk could not list since the cursor last moved, which may lie in an earlier
    // scope left unreconciled. Each is counted as a problem once the cursor moves past it or the
    // scan ends, since until then a resume lists it again.
    let mut unlisted_folders = 0;

    for (scope_index, scope) in from_cursor(scan) {
        if ctx.stopping() {
            break;
        }

        if !root_answers(&scope.root) {
            unanswered.push(scope);
            continue;
        }

        let resume_after = match &scan.cursor {
            Some(c) if c.scope_index == scope_index => c.after_directory.clone(),
            _ => None,
        };
        let mut sidecars = SidecarResolver::new(&scope.root);

        // A scope reached through a symlink below the root, as the watcher or a plugin may
        // name one, is skipped whole with a problem: nothing is walked and nothing reconciled.
        // One that no longer exists, or lies in an excluded folder, goes on to a walk that lists
        // nothing, so reconcile marks what was indexed under it missing. Excluded content is
        // never reported, so a link in an excluded folder is not either.
        if scope.path.exists()
            && !excluded_scope(&scope, &excludes)
            && let Err(error) = without_links(&scope.root, &scope.path)
        {
            writer.batch.problems.push(problems::from_io(&scope.path, &error));
            scan.progress.problems += 1 + std::mem::take(&mut unlisted_folders);
            scan.cursor = Some(Cursor { scope_index: scope_index + 1, after_directory: None });
            writer.flush(ctx, scan)?;
            continue;
        }

        let mut walk = Walk::new(&scope, &excludes, resume_after.as_deref());
        let walked = walk.by_ref().try_for_each(|item| {
            writer.checkpoint(ctx, scan)?;
            match item {
                WalkItem::Error(err) => {
                    writer.batch.problems.push(problems::from_io(&err.path, &err.error));
                    unlisted_folders += 1;
                }
                WalkItem::Directory(dir) => {
                    process_directory(ctx, scan, &scope, &dir, &mut sidecars, writer)?;
                    scan.cursor = Some(Cursor { scope_index, after_directory: Some(dir.path) });
                    scan.progress.problems += std::mem::take(&mut unlisted_folders);
                }
            }
            Ok(())
        });
        // The batch in hand is flushed whether the walk finished or was asked to stop.
        if let Err(Halt::Ended) = walked {
            return walked;
        }
        writer.flush(ctx, scan)?;
        if walked.is_err() {
            break;
        }

        // Stage 7: reconcile. Only after a complete walk of a root that still answers, and never
        // under what the walk could not read. Problems at paths that no longer exist go too.
        // A root that has gone leaves the cursor in this scope, so the unlisted folders stay
        // uncounted until a later scope moves the cursor or the scan ends, and the scope is left
        // for a retry to walk again, since a later scope may move the cursor past it.
        if !root_answers(&scope.root) {
            unanswered.push(scope);
            continue;
        }
        let reconciled = reconcile(ctx, &scope, scan.id, walk.unreadable());
        let (missing, vanished) = reconciled.map_err(|e| writer.fail(ctx, scan, &e))?;
        scan.progress.missing += missing;
        writer.batch.cleared = vanished;
        scan.cursor = Some(Cursor { scope_index: scope_index + 1, after_directory: None });
        scan.progress.problems += std::mem::take(&mut unlisted_folders);
        writer.flush(ctx, scan)?;
    }

    // A shutdown leaves the row as the last flush wrote it, running at its cursor, unless the
    // scan was cancelled too.
    let cancelled = ctx.cancelled();
    if ctx.shutdown.load(Ordering::SeqCst) && !cancelled {
        return Ok(());
    }
    // A scan that ends never resumes, so it counts the unlisted folders it carries.
    scan.progress.problems += unlisted_folders;
    scan.state = if cancelled {
        ScanState::Cancelled
    } else if !unanswered.is_empty() {
        ScanState::Suspended
    } else {
        ScanState::Completed
    };
    scan.finished_at = Some(now_ms());
    scan.progress.current_path = None;
    writer.save(ctx, scan)
}

/// Stage 7 on `scope`, walked whole by scan `scan_id`: marks the tracks the scan did not see as
/// missing, other than those under the `unreadable` paths, and returns how many, with the paths
/// under `scope` that have a problem but no longer exist.
fn reconcile(
    ctx: &ScanContext<'_>,
    scope: &Scope,
    scan_id: ScanId,
    unreadable: &[PathBuf],
) -> Result<(i64, Vec<PathBuf>), StoreError> {
    // The read goes first, so a failure cannot leave tracks marked missing that the scan has
    // not counted.
    let problems = ctx.store.problems_under(ctx.library.id, scope)?;
    let missing = ctx.store.mark_missing_unseen(ctx.library.id, scope, scan_id, unreadable)?;
    let vanished =
        problems.into_iter().filter(|p| fs::symlink_metadata(p).is_err_and(|e| gone(&e)));
    Ok((missing, vanished.collect()))
}

/// Whether `root` is available, logging why not. Nothing under a root that is not may be
/// touched (`requirements/scanning.md` §7, `design/scanning.md` §8).
fn root_answers(root: &Path) -> bool {
    match check_root(root) {
        Ok(()) => return true,
        Err(RootUnavailable::Inaccessible(e)) => {
            tracing::warn!(root = %root.display(), error = %e, "root unavailable; suspending")
        }
        Err(RootUnavailable::Empty) => {
            tracing::warn!(root = %root.display(), "root listed empty; treating as unavailable")
        }
    }
    false
}

/// Runs every audio candidate in `dir` through stages 2 to 5 into the writer's batch, with a
/// checkpoint before each, clears any problem on `dir` itself, since it listed, and records a
/// problem for each symlink the walk skipped and each entry it could not read. Returns
/// [`Halt::Stop`] if the scan is asked to stop partway, or [`Halt::Ended`] once a store failure
/// has ended it.
///
/// The caller moves the cursor past `dir` only once all of it is processed, so a scan stopped
/// partway walks it again from the start on resume. A track added or updated before the stop is
/// found unchanged then, so its counts go to the progress at once. The files seen, the problem
/// files and the entry problems would be counted again, so they go in only once `dir` is
/// complete, or when a cancel stops the scan in it, since a cancelled scan never resumes.
fn process_directory(
    ctx: &ScanContext<'_>,
    scan: &mut Scan,
    scope: &Scope,
    dir: &Directory,
    sidecars: &mut SidecarResolver,
    writer: &mut Writer,
) -> Result<(), Halt> {
    scan.progress.current_path = Some(dir.path.clone());
    writer.batch.cleared.push(dir.path.clone());
    let mut entry_problems = 0;
    for file in dir.skipped_links.iter().chain(&dir.unreadable) {
        writer.batch.problems.push(problems::from_io(&file.path, &file.error));
        entry_problems += 1;
    }
    sidecars.offer_listing(&dir.path, &dir.files);
    let mut resolved = None;
    let (mut seen, mut problem_files) = (0, 0);
    let mut halted = Ok(());

    for candidate in &dir.candidates {
        halted = writer.checkpoint(ctx, scan);
        if halted.is_err() {
            break;
        }
        seen += 1;
        let processed = process_file(ctx, scope, dir, candidate, sidecars, &mut resolved);
        let outcome = match processed {
            Ok(outcome) => outcome,
            Err(error) => {
                halted = Err(writer.fail(ctx, scan, &error));
                break;
            }
        };
        let batch = &mut writer.batch;
        match outcome {
            FileOutcome::Unchanged { track_id } => batch.touched.push(track_id),
            FileOutcome::Returned { track_id } => {
                batch.returned.push(track_id);
                batch.cleared.push(candidate.path.clone());
            }
            FileOutcome::Added(record) => {
                scan.progress.files_processed += 1;
                scan.progress.added += 1;
                batch.cleared.push(candidate.path.clone());
                batch.upserts.push(*record);
            }
            FileOutcome::Updated(record) => {
                scan.progress.files_processed += 1;
                scan.progress.updated += 1;
                batch.cleared.push(candidate.path.clone());
                batch.upserts.push(*record);
            }
            FileOutcome::Problem(problem) => {
                problem_files += 1;
                batch.problems.push(problem);
            }
            FileOutcome::Unread { track_id, missing, changed, problem } => {
                problem_files += 1;
                batch.problems.push(problem);
                let seen = if missing { &mut batch.returned } else { &mut batch.touched };
                seen.push(track_id);
                if changed {
                    batch.changed_unread.push(track_id);
                }
            }
        }
    }
    if halted.is_ok() || (matches!(halted, Err(Halt::Stop)) && ctx.cancelled()) {
        scan.progress.files_seen += seen;
        scan.progress.files_processed += problem_files;
        scan.progress.problems += problem_files + entry_problems;
    }
    halted
}

#[cfg(test)]
mod tests;
