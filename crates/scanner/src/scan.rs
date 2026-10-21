//! The pipeline over one scan (`design/scanning.md` §2, §7, §8).

use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use jewelcase_core::sort_key;

use crate::confine::without_links;
use crate::discover::{
    Candidate, Directory, RootUnavailable, Walk, WalkItem, check_root, compile_excludes, gone,
};
use crate::governor::Governor;
use crate::identity::{self, FileFacts, Identity};
use crate::problems;
use crate::sidecar::SidecarResolver;
use crate::store::{Batch, IndexedFile, IndexedLyrics, Store};
use crate::tags;
use crate::types::*;

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
    /// Set by the queue to stop the scan after the directory in hand and end it cancelled.
    pub cancel: &'a AtomicBool,
    /// Set by the queue to stop the scan at the same point but leave it running at its last
    /// flushed cursor, so the next start resumes it.
    pub shutdown: &'a AtomicBool,
    pub options: &'a ScanOptions,
}

impl ScanContext<'_> {
    /// Whether the queue has asked the scan to stop, to cancel it or to shut down.
    fn stopping(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) || self.shutdown.load(Ordering::SeqCst)
    }
}

/// The cursor and progress the store holds for a scan.
type Persisted = (Option<Cursor>, ScanProgress);

/// Run `scan` to completion, cancellation, or suspension, or until a shutdown, which leaves it
/// running at its last flushed cursor for the next start to resume. A panic ends it at that
/// cursor through [`suspend`], so the caller carries on. The scan row is updated on every flush;
/// `scan` is left in the state the row holds.
pub fn run_scan(ctx: &ScanContext<'_>, scan: &mut Scan) {
    let mut persisted = (scan.cursor.clone(), scan.progress.clone());
    // Asserted unwind safe because a panic can leave `scan`'s cursor and progress ahead of the
    // store, and `suspend` replaces both from `persisted`, which changes only as a whole, after a
    // flush, and sets the state itself. Everything else the scan touches is shared through `ctx`
    // and keeps its own guarantees: the store commits a batch whole or not at all.
    let scanned = panic::catch_unwind(AssertUnwindSafe(|| scan_scopes(ctx, scan, &mut persisted)));
    if scanned.is_err() {
        tracing::error!(scan = scan.id, "scan panicked; ending it at its last flushed cursor");
        suspend(ctx, scan, &persisted);
    }
}

/// [`run_scan`] without the panic guard, keeping `persisted` current as it flushes.
fn scan_scopes(ctx: &ScanContext<'_>, scan: &mut Scan, persisted: &mut Persisted) {
    let excludes = match compile_excludes(&ctx.library.excludes) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(error = %e, "invalid exclude pattern; scanning with none");
            compile_excludes(&[]).expect("empty globset")
        }
    };

    scan.state = ScanState::Running;
    if scan.started_at.is_none() {
        scan.started_at = Some(SystemTime::now());
    }
    ctx.store.update_scan(scan);

    let start_scope = scan.cursor.as_ref().map(|c| c.scope_index).unwrap_or(0);
    let mut suspended = false;

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
        let mut batch = Batch::new(scan.id);
        let mut last_flush = Instant::now();
        let mut stopped = false;

        // A scope reached through a symlink below the root, as the watcher or a plugin may
        // name one, is skipped whole: nothing is walked and nothing reconciled. One that no
        // longer exists is walked, so what was under it is marked missing.
        if scope.path.exists()
            && let Err(error) = without_links(&scope.root, &scope.path)
        {
            batch.problems.push(problems::from_io(&scope.path, &error));
            scan.progress.problems += 1;
            scan.cursor = Some(Cursor { scope_index: scope_index + 1, after_directory: None });
            if !flush(ctx, scan, &mut batch, persisted) {
                return;
            }
            continue;
        }

        let mut walk = Walk::new(&scope, &excludes, resume_after.as_deref());
        for item in walk.by_ref() {
            ctx.governor.wait_if_paused();
            if ctx.stopping() {
                stopped = true;
                break;
            }
            match item {
                WalkItem::Error(err) => {
                    batch.problems.push(problems::from_io(&err.path, &err.error));
                    scan.progress.problems += 1;
                }
                WalkItem::Directory(dir) => {
                    process_directory(ctx, scan, &scope, &dir, &mut sidecars, &mut batch);
                    scan.cursor =
                        Some(Cursor { scope_index, after_directory: Some(dir.path.clone()) });
                    scan.progress.current_path = Some(dir.path);
                }
            }
            if batch.len() >= ctx.options.batch_size
                || last_flush.elapsed() >= ctx.options.batch_interval
            {
                if !flush(ctx, scan, &mut batch, persisted) {
                    return;
                }
                last_flush = Instant::now();
            }
        }
        if !flush(ctx, scan, &mut batch, persisted) {
            return;
        }

        if stopped {
            break;
        }

        // Stage 7: reconcile. Only after a complete walk of a root that still answers, and never
        // under what the walk could not read. Problems at paths that no longer exist go too.
        if !root_answers(&scope.root) {
            suspended = true;
            continue;
        }
        let missing =
            ctx.store.mark_missing_unseen(&ctx.library.id, &scope, scan.id, walk.unreadable());
        scan.progress.missing += missing as u64;
        batch.cleared = ctx
            .store
            .problems_under(&ctx.library.id, &scope)
            .into_iter()
            .filter(|path| fs::symlink_metadata(path).is_err_and(|error| gone(&error)))
            .collect();
        scan.cursor = Some(Cursor { scope_index: scope_index + 1, after_directory: None });
        if !flush(ctx, scan, &mut batch, persisted) {
            return;
        }
    }

    // A shutdown leaves the row as the last flush wrote it, running at its cursor, unless the
    // scan was cancelled too.
    let cancelled = ctx.cancel.load(Ordering::SeqCst);
    if ctx.shutdown.load(Ordering::SeqCst) && !cancelled {
        return;
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

/// Persist the batch and the scan row. On a store failure the scan ends at
/// its last persisted cursor through [`suspend`] rather than reporting
/// success over lost work (`requirements/scanning.md` §10: problems are
/// honest). Returns whether the scan may continue.
fn flush(
    ctx: &ScanContext<'_>,
    scan: &mut Scan,
    batch: &mut Batch,
    persisted: &mut Persisted,
) -> bool {
    if !batch.is_empty() {
        let full = std::mem::replace(batch, Batch::new(scan.id));
        if let Err(e) = ctx.store.apply(&ctx.library.id, full) {
            tracing::error!(scan = scan.id, error = %e, "batch not persisted; suspending scan");
            suspend(ctx, scan, persisted);
            return false;
        }
    }
    *persisted = (scan.cursor.clone(), scan.progress.clone());
    ctx.store.update_scan(scan);
    true
}

/// Ends `scan` at the cursor and progress the store holds and saves its row: suspended, so a
/// resume retries what was not persisted, or cancelled if a cancel was asked for, since a
/// cancelled scan never resumes.
fn suspend(ctx: &ScanContext<'_>, scan: &mut Scan, persisted: &Persisted) {
    scan.cursor = persisted.0.clone();
    scan.progress = persisted.1.clone();
    scan.progress.current_path = None;
    scan.state =
        if ctx.cancel.load(Ordering::SeqCst) { ScanState::Cancelled } else { ScanState::Suspended };
    scan.finished_at = Some(SystemTime::now());
    ctx.store.update_scan(scan);
}

/// Runs every audio candidate in `dir` through stages 2 to 5 into `batch`, clears any problem
/// on `dir` itself, since it listed, and records a problem for each symlink the walk skipped
/// and each entry it could not read.
fn process_directory(
    ctx: &ScanContext<'_>,
    scan: &mut Scan,
    scope: &Scope,
    dir: &Directory,
    sidecars: &mut SidecarResolver,
    batch: &mut Batch,
) {
    batch.cleared.push(dir.path.clone());
    for failure in dir.skipped_links.iter().chain(&dir.unreadable) {
        scan.progress.problems += 1;
        batch.problems.push(problems::from_io(&failure.path, &failure.error));
    }
    sidecars.offer_listing(&dir.path, &dir.files);
    let mut resolved = None;

    for candidate in &dir.candidates {
        scan.progress.files_seen += 1;
        let outcome = process_file(ctx, scan, scope, dir, candidate, sidecars, &mut resolved);
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
                scan.progress.files_processed += 1;
                scan.progress.problems += 1;
                batch.problems.push(problem);
            }
        }
    }
}

/// Whether a lyrics file has appeared beside a track that had none, or gone from one
/// that had it (`requirements/scanning.md` §7). Checked against the directory listing
/// discovery already holds, so an unchanged library still costs no extra I/O. Embedded
/// lyrics win (§3.4), so a sidecar next to them changes nothing.
fn lyrics_file_changed(ix: &IndexedFile, candidate: &Candidate, dir: &Directory) -> bool {
    let present = SidecarResolver::lyrics_path_for(&candidate.path, &dir.files).is_some();
    match ix.lyrics {
        IndexedLyrics::None => present,
        IndexedLyrics::Sidecar => !present,
        IndexedLyrics::Embedded => false,
    }
}

fn process_file(
    ctx: &ScanContext<'_>,
    scan: &Scan,
    scope: &Scope,
    dir: &Directory,
    candidate: &Candidate,
    sidecars: &mut SidecarResolver,
    resolved: &mut Option<crate::sidecar::Sidecars>,
) -> FileOutcome {
    // Stage 2: skip. Path, size, and mtime unchanged means done without
    // opening the file. This check carries the incremental budget.
    let indexed = ctx.store.lookup(&ctx.library.id, &candidate.path);
    if let Some(ix) = &indexed
        && ix.size == candidate.size
        && ix.mtime_ms == candidate.mtime_ms
        && !lyrics_file_changed(ix, candidate, dir)
    {
        return if ix.missing {
            FileOutcome::Returned { track_id: ix.track_id }
        } else {
            FileOutcome::Unchanged { track_id: ix.track_id }
        };
    }

    // Stage 3: read headers.
    let read = match tags::read(&candidate.path, candidate.size) {
        Ok(r) => r,
        Err(e) => return FileOutcome::Problem(problems::from_read_error(&candidate.path, &e)),
    };

    // Stage 4: identify.
    let facts = FileFacts {
        root: &scope.root,
        path: &candidate.path,
        size: candidate.size,
        mtime_ms: candidate.mtime_ms,
        tags: &read.tags,
        properties: &read.properties,
        indexed: indexed.as_ref(),
    };
    let identity = identity::identify(&facts, &ctx.library.id, ctx.store);

    // Stage 5: sidecars, once per directory. Embedded art wins over a
    // sidecar; sidecar lyrics apply only when the file has none.
    let side = resolved.get_or_insert_with(|| sidecars.resolve(&dir.path)).clone();
    let artwork = match read.embedded_art {
        Some(info) => Some(Artwork { source: ArtworkSource::Embedded, info }),
        None => {
            side.cover.map(|c| Artwork { source: ArtworkSource::Sidecar(c.path), info: c.info })
        }
    };
    let lyrics_sidecar = if read.tags.lyrics.is_none() {
        SidecarResolver::lyrics_for(&candidate.path, &dir.files)
    } else {
        None
    };

    let title = read.tags.title.clone().unwrap_or_else(|| filename_title(&candidate.path));
    let sort = SortKeys {
        title: sort_key(&title, read.tags.title_sort.as_deref()),
        artist: sort_key(
            read.tags.artists.first().map(String::as_str).unwrap_or(""),
            read.tags.artist_sort.as_deref(),
        ),
        album: sort_key(read.tags.album.as_deref().unwrap_or(""), read.tags.album_sort.as_deref()),
        album_artist: sort_key(
            read.tags.album_artists.first().map(String::as_str).unwrap_or(""),
            read.tags.album_artist_sort.as_deref(),
        ),
    };

    let record = TrackRecord {
        id: None,
        root: scope.root.clone(),
        path: candidate.path.clone(),
        size: candidate.size,
        mtime_ms: candidate.mtime_ms,
        tags: read.tags,
        properties: read.properties,
        artwork,
        artist_image: side.artist_image,
        artist_biography: side.biography,
        lyrics_sidecar,
        sort,
        last_seen_scan: scan.id,
    };

    match identity {
        Identity::New => FileOutcome::Added(Box::new(record)),
        Identity::Updated { track_id } | Identity::Moved { track_id, .. } => {
            FileOutcome::Updated(Box::new(TrackRecord { id: Some(track_id), ..record }))
        }
    }
}

#[cfg(test)]
mod tests;
