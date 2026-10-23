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
mod reconcile;
mod row;
mod writer;

pub use row::{Cursor, Scan, ScanId, ScanProgress, ScanState, Trigger};

/// When a scan flushes its batch. The scanner always runs with the defaults, the design's
/// starting points; tests choose others to flush at the points they check.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Flush a batch at this many outcomes at most...
    pub batch_size: usize,
    /// ...or after this long, whichever comes first, so progress is visible
    /// and content appears within seconds.
    pub batch_interval: Duration,
    /// How long a batch should hold the store's writer, which interactive writes wait behind.
    /// Each batch is sized to apply in about this long at the rate the last one applied...
    pub batch_time: Duration,
    /// ...but holds at least this many outcomes, or `batch_size` if that is fewer, since part of
    /// what a batch costs, such as its commit and the albums it touches, does not shrink with it.
    /// A scan's first batch is this size.
    pub min_batch: usize,
}

impl Default for ScanOptions {
    /// The design's values (`design/scanning.md` §7): batches of 16 to 250 outcomes, each sized
    /// to apply in about 50 ms, flushed at least every 2 seconds.
    fn default() -> Self {
        ScanOptions {
            batch_size: 250,
            batch_interval: Duration::from_secs(2),
            batch_time: Duration::from_millis(50),
            min_batch: 16,
        }
    }
}

/// What a scan needs from its surroundings.
pub struct ScanContext<'a> {
    pub store: &'a dyn Store,
    pub library: &'a LibraryConfig,
    /// Set by the queue to stop the scan after the file in hand and leave it running at its last
    /// flushed cursor, so the next start resumes it.
    pub shutdown: &'a AtomicBool,
    pub options: &'a ScanOptions,
}

impl ScanContext<'_> {
    /// Whether the queue has asked the scan to stop for a shutdown.
    fn stopping(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
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

/// Run `scan` to completion or suspension, or until a shutdown, which leaves it
/// running at its last flushed cursor for the next start to resume. A panic ends it at that
/// cursor through [`Writer::suspend`], so the caller carries on. The scan row is updated on every
/// flush; `scan` is left in the state the row holds, unless the store could not save it.
///
/// Returns what a suspended scan left undone, and nothing for any other end. That leaves out what
/// a panic or an exclude pattern that does not compile left unwalked, since a retry would panic
/// or suspend again, but not a root found not answering before a panic.
pub fn run_scan(ctx: &ScanContext<'_>, scan: &mut Scan) -> Undone {
    let mut writer = Writer::new(scan, ctx.options);
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
        // its folder as empty, so reconcile marks what was indexed under it missing. Excluded
        // content is never reported, so a link in an excluded folder is not either.
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

        let walked = Walk::new(&scope, &excludes, resume_after.as_deref()).try_for_each(|item| {
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
        // The batch in hand is flushed whether the walk finished, was asked to stop, or found
        // its root gone.
        if let Err(Halt::Ended) = walked {
            return walked;
        }
        writer.flush(ctx, scan)?;
        if let Err(Halt::Stop) = walked {
            break;
        }

        // Stage 7 runs on each folder as the walk lists it (`process_directory`). What remains
        // is clearing the problems at paths that no longer exist, only after a complete walk of a
        // root that still answers.
        // A root that has gone leaves the cursor in this scope, so the unlisted folders stay
        // uncounted until a later scope moves the cursor or the scan ends, and the scope is left
        // for a retry to walk again, since a later scope may move the cursor past it.
        if walked.is_err() || !root_answers(&scope.root) {
            unanswered.push(scope);
            continue;
        }
        let gone = gone_problems(ctx, &scope).map_err(|e| writer.fail(ctx, scan, &e))?;
        writer.batch.cleared = gone;
        scan.cursor = Some(Cursor { scope_index: scope_index + 1, after_directory: None });
        scan.progress.problems += std::mem::take(&mut unlisted_folders);
        writer.flush(ctx, scan)?;
    }

    // A shutdown leaves the row as the last flush wrote it, running at its cursor.
    if ctx.stopping() {
        return Ok(());
    }
    // A scan that ends never resumes, so it counts the unlisted folders it carries.
    scan.progress.problems += unlisted_folders;
    scan.state = if !unanswered.is_empty() { ScanState::Suspended } else { ScanState::Completed };
    scan.finished_at = Some(now_ms());
    scan.progress.current_path = None;
    writer.save(ctx, scan)
}

/// The paths under `scope` with a recorded problem that no longer exist.
fn gone_problems(ctx: &ScanContext<'_>, scope: &Scope) -> Result<Vec<PathBuf>, StoreError> {
    let problems = ctx.store.problems_under(ctx.library.id, scope)?;
    let gone = problems.into_iter().filter(|p| fs::symlink_metadata(p).is_err_and(|e| gone(&e)));
    Ok(gone.collect())
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
/// checkpoint before each, against what the index holds in `dir`, read once for all of them.
/// Then reconciles `dir` (stage 7): the tracks in it whose files it no longer lists, and every
/// track under a folder it no longer lists, go missing through the batch ([`reconcile::mark`]),
/// once the root is found still answering. Clears any problem on `dir` itself if it listed, and
/// records a problem for each symlink the walk skipped and each entry it could not read. Returns
/// [`Halt::Stop`] if the scan is asked to stop partway, [`Halt::Unanswered`] if the root no
/// longer answers, or [`Halt::Ended`] once a store failure has ended it.
///
/// The caller moves the cursor past `dir` only once all of it is processed, so a scan stopped
/// partway walks and reconciles it again from the start on resume. A track added or updated
/// before the stop is found unchanged then, so its counts go to the progress at once. The files
/// seen, the problem files and the entry problems would be counted again, so they go in only
/// once `dir` is complete, or when the scan stops in it never to resume, its root gone.
fn process_directory(
    ctx: &ScanContext<'_>,
    scan: &mut Scan,
    scope: &Scope,
    dir: &Directory,
    sidecars: &mut SidecarResolver,
    writer: &mut Writer,
) -> Result<(), Halt> {
    scan.progress.current_path = Some(dir.path.clone());
    let indexed = ctx.store.indexed_in(ctx.library.id, &dir.path);
    let mut indexed = indexed.map_err(|e| writer.fail(ctx, scan, &e))?;
    if dir.listed {
        writer.batch.cleared.push(dir.path.clone());
    }
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
        let known = indexed.take(&candidate.path);
        let outcome = process_file(scope, dir, candidate, known, sidecars, &mut resolved);
        let batch = &mut writer.batch;
        match outcome {
            FileOutcome::Unchanged => {}
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
                if missing {
                    batch.returned.push(track_id);
                }
                if changed {
                    batch.changed_unread.push(track_id);
                }
            }
        }
    }
    if halted.is_ok() {
        let (missing, vanished) = reconcile::unlisted(dir, indexed);
        // A root that went during the walk lists every folder after it as gone, so nothing is
        // marked missing unless it still answers (`requirements/scanning.md` §7).
        let marks = !missing.is_empty() || !vanished.is_empty();
        halted = if marks && !root_answers(&scope.root) {
            Err(Halt::Unanswered)
        } else {
            reconcile::mark(ctx, scan, writer, missing, vanished)
        };
    }
    // Whether no resume walks `dir` again, so its counts go in here.
    if matches!(halted, Ok(()) | Err(Halt::Unanswered)) {
        scan.progress.files_seen += seen;
        scan.progress.files_processed += problem_files;
        scan.progress.problems += problem_files + entry_problems;
    }
    halted
}
