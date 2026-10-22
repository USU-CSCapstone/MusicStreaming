//! Stages 2 to 5 on one file (`design/scanning.md` §2).

use jewelcase_core::sort_key;

use crate::discover::{Candidate, Directory};
use crate::identity::{self, Identity};
use crate::problems::{self, Problem};
use crate::scope::Scope;
use crate::sidecar::SidecarResolver;
use crate::store::{IndexedFile, IndexedLyrics};
use crate::tags;
use crate::track::{Artwork, ArtworkSource, SortKeys, TrackId, TrackRecord, filename_title};

/// Per-file result of the pipeline (`design/scanning.md` §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOutcome {
    /// A file the index holds as it is, which the scan has nothing to write for.
    Unchanged,
    /// A missing track whose file is back, byte-identical.
    Returned {
        track_id: TrackId,
    },
    Added(Box<TrackRecord>),
    Updated(Box<TrackRecord>),
    /// A file with no track that could not be read.
    Problem(Problem),
    /// A known track's file, there but unreadable, with its `problem`. `changed` is whether the
    /// file's size or mtime differs from the index, or the index has forgotten its mtime.
    Unread {
        track_id: TrackId,
        missing: bool,
        changed: bool,
        problem: Problem,
    },
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

/// Runs `candidate`, one of `dir`'s files, through stages 2 to 5, given what the index holds
/// at its path, `indexed`: what the scan does with it.
pub(super) fn process_file(
    scope: &Scope,
    dir: &Directory,
    candidate: &Candidate,
    indexed: Option<IndexedFile>,
    sidecars: &mut SidecarResolver,
    resolved: &mut Option<crate::sidecar::Sidecars>,
) -> FileOutcome {
    // Stage 2: skip. Path, size, and mtime unchanged means done without
    // opening the file. This check carries the incremental budget.
    if let Some(ix) = &indexed
        && ix.size == candidate.size
        && ix.mtime_ms == Some(candidate.mtime_ms)
        && !lyrics_file_changed(ix, candidate, dir)
    {
        return if ix.missing {
            FileOutcome::Returned { track_id: ix.track_id }
        } else {
            FileOutcome::Unchanged
        };
    }

    // Stage 3: read headers. A known file that cannot be read keeps its track, as a problem.
    let read = match tags::read(&candidate.path) {
        Ok(r) => r,
        Err(e) => {
            let problem = problems::from_read_error(&candidate.path, &e);
            return match indexed {
                Some(ix) => FileOutcome::Unread {
                    track_id: ix.track_id,
                    missing: ix.missing,
                    changed: (ix.size, ix.mtime_ms) != (candidate.size, Some(candidate.mtime_ms)),
                    problem,
                },
                None => FileOutcome::Problem(problem),
            };
        }
    };

    // Stage 4: identify.
    let identity = identity::identify(indexed.as_ref());

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
    };

    match identity {
        Identity::New => FileOutcome::Added(Box::new(record)),
        Identity::Updated { track_id } => {
            FileOutcome::Updated(Box::new(TrackRecord { id: Some(track_id), ..record }))
        }
    }
}
