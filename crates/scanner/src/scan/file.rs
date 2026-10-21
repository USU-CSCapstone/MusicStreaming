//! Stages 2 to 5 on one file (`design/scanning.md` §2).

use jewelcase_core::sort_key;

use super::ScanContext;
use crate::discover::{Candidate, Directory};
use crate::identity::{self, FileFacts, Identity};
use crate::problems;
use crate::sidecar::SidecarResolver;
use crate::store::{IndexedFile, IndexedLyrics};
use crate::tags;
use crate::types::*;

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

/// Runs `candidate`, one of `dir`'s files, through stages 2 to 5: what the scan does with it.
pub(super) fn process_file(
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
