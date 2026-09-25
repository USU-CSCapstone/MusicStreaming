//! The pipeline over one scan (`design/scanning.md` §2, §7, §8).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use jewelcase_core::sort::sort_key;

use crate::discover::{
    self, Candidate, Directory, RootUnavailable, Walk, WalkItem, check_root, compile_excludes,
};
use crate::governor::Governor;
use crate::identity::{self, FileFacts, Identity};
use crate::problems;
use crate::sidecar::SidecarResolver;
use crate::store::{Batch, Store};
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
        ScanOptions {
            batch_size: 250,
            batch_interval: Duration::from_secs(2),
        }
    }
}

/// What a scan needs from its surroundings.
pub struct ScanContext<'a> {
    pub store: &'a dyn Store,
    pub library: &'a LibraryConfig,
    pub governor: &'a Governor,
    /// Set by the queue to stop at the next batch boundary.
    pub cancel: &'a AtomicBool,
    pub options: &'a ScanOptions,
}

/// Run `scan` to completion, cancellation, or suspension. The scan row is
/// updated on every flush; `scan` is left in its final state.
pub fn run_scan(ctx: &ScanContext<'_>, scan: &mut Scan) {
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
        if ctx.cancel.load(Ordering::SeqCst) {
            break;
        }
        let scope = scan.scopes[scope_index].clone();

        // Storage unavailable: suspend and touch nothing under this root
        // (`requirements/scanning.md` §7, `design/scanning.md` §8).
        if let Err(why) = check_root(&scope.root) {
            match why {
                RootUnavailable::Inaccessible(e) => {
                    tracing::warn!(root = %scope.root.display(), error = %e, "root unavailable; suspending")
                }
                RootUnavailable::Empty => {
                    tracing::warn!(root = %scope.root.display(), "root listed empty; treating as unavailable")
                }
            }
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
        let mut cancelled = false;

        let walk = Walk::new(&scope, &excludes, resume_after.as_deref());
        for item in walk {
            ctx.governor.wait_if_paused();
            if ctx.cancel.load(Ordering::SeqCst) {
                cancelled = true;
                break;
            }
            match item {
                WalkItem::Error(err) => {
                    batch
                        .problems
                        .push(problems::from_io(&err.path, &err.error));
                    scan.progress.problems += 1;
                }
                WalkItem::Directory(dir) => {
                    process_directory(ctx, scan, &scope, &dir, &mut sidecars, &mut batch);
                    scan.cursor = Some(Cursor {
                        scope_index,
                        after_directory: Some(dir.path.clone()),
                    });
                    scan.progress.current_path = Some(dir.path);
                }
            }
            if batch.len() >= ctx.options.batch_size
                || last_flush.elapsed() >= ctx.options.batch_interval
            {
                flush(ctx, scan, &mut batch);
                last_flush = Instant::now();
            }
        }
        flush(ctx, scan, &mut batch);

        if cancelled {
            break;
        }

        // Stage 7: reconcile. Only after a complete, uncancelled walk of a
        // root that answered.
        let missing = ctx
            .store
            .mark_missing_unseen(&ctx.library.id, &scope, scan.id);
        scan.progress.missing += missing as u64;
        scan.cursor = Some(Cursor {
            scope_index: scope_index + 1,
            after_directory: None,
        });
        ctx.store.update_scan(scan);
    }

    scan.state = if ctx.cancel.load(Ordering::SeqCst) {
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

fn flush(ctx: &ScanContext<'_>, scan: &mut Scan, batch: &mut Batch) {
    if !batch.is_empty() {
        let full = std::mem::replace(batch, Batch::new(scan.id));
        ctx.store.apply(&ctx.library.id, full);
    }
    ctx.store.update_scan(scan);
}

fn process_directory(
    ctx: &ScanContext<'_>,
    scan: &mut Scan,
    scope: &Scope,
    dir: &Directory,
    sidecars: &mut SidecarResolver,
    batch: &mut Batch,
) {
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
    {
        return if ix.missing {
            FileOutcome::Returned {
                track_id: ix.track_id,
            }
        } else {
            FileOutcome::Unchanged {
                track_id: ix.track_id,
            }
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

    // Stage 5: sidecars, once per directory.
    let side = resolved
        .get_or_insert_with(|| sidecars.resolve(&dir.path))
        .clone();
    let artwork = if read.tags.has_embedded_art {
        Some(ArtworkSource::Embedded)
    } else {
        side.cover.map(ArtworkSource::Sidecar)
    };
    let lyrics_sidecar = if read.tags.lyrics.is_none() {
        SidecarResolver::lyrics_for(&candidate.path, &dir.files)
    } else {
        None
    };

    let title = read
        .tags
        .title
        .clone()
        .unwrap_or_else(|| filename_title(&candidate.path));
    let sort = SortKeys {
        title: sort_key(&title, read.tags.title_sort.as_deref()),
        artist: sort_key(
            read.tags.artists.first().map(String::as_str).unwrap_or(""),
            read.tags.artist_sort.as_deref(),
        ),
        album: sort_key(
            read.tags.album.as_deref().unwrap_or(""),
            read.tags.album_sort.as_deref(),
        ),
        album_artist: sort_key(
            read.tags
                .album_artists
                .first()
                .map(String::as_str)
                .unwrap_or(""),
            read.tags.album_artist_sort.as_deref(),
        ),
    };

    let record = TrackRecord {
        id: None,
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
            FileOutcome::Updated(Box::new(TrackRecord {
                id: Some(track_id),
                ..record
            }))
        }
    }
}

#[allow(dead_code)]
fn _assert_discover_used(_: &discover::Directory) {}
