//! The pipeline over one scan (`design/scanning.md` §2, §7, §8).

use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use crate::confine::without_links;
use crate::discover::{
    Directory, RootUnavailable, Walk, WalkItem, check_root, compile_excludes, excluded_scope, gone,
};
use crate::governor::Governor;
use crate::problems;
use crate::sidecar::SidecarResolver;
use crate::store::Store;
use crate::types::*;
use file::process_file;
use writer::{Halt, Writer};

mod file;
mod writer;

/// Tunables. Defaults are the design's starting points; benchmarks decide
/// the shipped values.
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
    pub governor: &'a Governor,
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

/// Run `scan` to completion, cancellation, or suspension, or until a shutdown, which leaves it
/// running at its last flushed cursor for the next start to resume. A panic ends it at that
/// cursor through [`Writer::suspend`], so the caller carries on. The scan row is updated on every
/// flush; `scan` is left in the state the row holds.
pub fn run_scan(ctx: &ScanContext<'_>, scan: &mut Scan) {
    let mut writer = Writer::new(scan);
    // Asserted unwind safe because a panic can leave `scan`'s cursor and progress ahead of the
    // store, and `suspend` replaces both from what the writer last persisted, which changes only
    // as a whole, after a flush, and sets the state itself; a batch left half filled is dropped.
    // Everything else the scan touches is shared through `ctx` and keeps its own guarantees: the
    // store commits a batch whole or not at all.
    let scanned = panic::catch_unwind(AssertUnwindSafe(|| scan_scopes(ctx, scan, &mut writer)));
    if scanned.is_err() {
        tracing::error!(scan = scan.id, "scan panicked; ending it at its last flushed cursor");
        writer.suspend(ctx, scan);
    }
}

/// [`run_scan`] without the panic guard, flushing through `writer`.
fn scan_scopes(ctx: &ScanContext<'_>, scan: &mut Scan, writer: &mut Writer) {
    // Leaving out a pattern that does not compile would index what the admin meant to exclude.
    let excludes = match compile_excludes(&ctx.library.excludes) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(scan = scan.id, error = %e, "invalid exclude pattern; suspending scan");
            writer.suspend(ctx, scan);
            return;
        }
    };

    scan.state = ScanState::Running;
    if scan.started_at.is_none() {
        scan.started_at = Some(SystemTime::now());
    }
    ctx.store.update_scan(scan);

    let start_scope = scan.cursor.as_ref().map(|c| c.scope_index).unwrap_or(0);
    let mut suspended = false;
    // Folders the walk could not list since the cursor last moved, which may lie in an earlier
    // scope left unreconciled. Each is counted as a problem once the cursor moves past it, since
    // until then a resume lists it again.
    let mut unlisted_folders = 0;

    for scope_index in start_scope..scan.scopes.len() {
        if ctx.stopping() {
            break;
        }
        let scope = scan.scopes[scope_index].clone();

        if !root_answers(&scope.root) {
            suspended = true;
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
            if writer.flush(ctx, scan).is_err() {
                return;
            }
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
        if matches!(walked, Err(Halt::Ended)) || writer.flush(ctx, scan).is_err() {
            return;
        }
        if walked.is_err() {
            break;
        }

        // Stage 7: reconcile. Only after a complete walk of a root that still answers, and never
        // under what the walk could not read. Problems at paths that no longer exist go too.
        // A root that has gone leaves the cursor in this scope, so the unlisted folders stay
        // uncounted: a resume lists them again, or a later scope counts them as it moves the
        // cursor.
        if !root_answers(&scope.root) {
            suspended = true;
            continue;
        }
        let missing =
            ctx.store.mark_missing_unseen(&ctx.library.id, &scope, scan.id, walk.unreadable());
        scan.progress.missing += missing as u64;
        writer.batch.cleared = ctx
            .store
            .problems_under(&ctx.library.id, &scope)
            .into_iter()
            .filter(|path| fs::symlink_metadata(path).is_err_and(|error| gone(&error)))
            .collect();
        scan.cursor = Some(Cursor { scope_index: scope_index + 1, after_directory: None });
        scan.progress.problems += std::mem::take(&mut unlisted_folders);
        if writer.flush(ctx, scan).is_err() {
            return;
        }
    }

    // A shutdown leaves the row as the last flush wrote it, running at its cursor, unless the
    // scan was cancelled too.
    let cancelled = ctx.cancelled();
    if ctx.shutdown.load(Ordering::SeqCst) && !cancelled {
        return;
    }
    if cancelled {
        // A cancelled scan never resumes, so it counts the unlisted folders it carries as it ends.
        scan.progress.problems += unlisted_folders;
    }
    scan.state = if cancelled {
        ScanState::Cancelled
    } else if suspended {
        ScanState::Suspended
    } else {
        ScanState::Completed
    };
    scan.finished_at = Some(SystemTime::now());
    scan.progress.current_path = None;
    ctx.store.update_scan(scan);
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
/// problem for each symlink the walk skipped and each entry it could not read. Returns the
/// checkpoint's [`Halt`] if the scan stops or ends partway.
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
    let entry_problems = dir.skipped_links.iter().chain(&dir.unreadable);
    writer.batch.problems.extend(entry_problems.map(|f| problems::from_io(&f.path, &f.error)));
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
        let outcome = process_file(ctx, scan, scope, dir, candidate, sidecars, &mut resolved);
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
        let entry_problems = (dir.skipped_links.len() + dir.unreadable.len()) as u64;
        scan.progress.files_seen += seen;
        scan.progress.files_processed += problem_files;
        scan.progress.problems += problem_files + entry_problems;
    }
    halted
}

#[cfg(test)]
mod tests;
