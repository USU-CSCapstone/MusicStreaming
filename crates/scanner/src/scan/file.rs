//! Stages 2 to 5 on one file (`design/scanning.md` §2).

use jewelcase_core::{name_key, sort_key};

use crate::discover::{Candidate, Directory};
use crate::identity::{self, Identity};
use crate::problems::{self, Problem};
use crate::scope::Scope;
use crate::sidecar::{SidecarResolver, Sidecars};
use crate::store::{IndexedFile, TagsReread};
use crate::tag_set::TagSet;
use crate::tags;
use crate::track::{Artwork, ArtworkSource, SortKeys, TrackId, TrackRecord, filename_title};

/// Per-file result of the pipeline (`design/scanning.md` §2). `Added` and `Updated` carry the
/// record of a file read: one the index holds no track for, or one it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOutcome {
    /// A file the index holds as it is, which the scan has nothing to write for.
    Unchanged,
    /// A missing track whose file is back with the size, mtime and sidecar stamp the index holds.
    Returned {
        track_id: TrackId,
    },
    Added(Box<TrackRecord>),
    Updated(Box<TrackRecord>),
    /// An unchanged file whose track was read with an older generation of tag types, read again
    /// for its tags alone.
    Reread(TagsReread),
    /// A file that could not be read. A track the index holds for it stays as it is.
    Problem(Problem),
}

/// Runs `candidate`, one of `dir`'s files, through stages 2 to 5, given what the index holds
/// at its path, `indexed`: what the scan does with it.
pub(super) fn process_file(
    scope: &Scope,
    dir: &Directory,
    candidate: &Candidate,
    indexed: Option<IndexedFile>,
    sidecars: &mut SidecarResolver,
    resolved: &mut Option<Sidecars>,
) -> FileOutcome {
    // Stage 2: skip. Path, size, mtime and sidecar stamp unchanged means done without opening
    // the file or any sidecar. This check carries the incremental budget. A file read before the
    // newest tag type was added is opened for its tags alone; one coming back from missing is
    // left to the next scan, which finds it present.
    let stamped = sidecars.stamp(&dir.path, &candidate.path, &dir.files);
    if let Some(ix) = &indexed
        && ix.size == candidate.size
        && ix.mtime_ms == candidate.mtime_ms
        && ix.sidecar_stamp == stamped.stamp
    {
        return if ix.missing {
            FileOutcome::Returned { track_id: ix.track_id }
        } else if ix.tags_generation < tags::tags_generation() {
            match tags::read(&candidate.path) {
                Ok(read) => FileOutcome::Reread(TagsReread {
                    track_id: ix.track_id,
                    tag_values: read.tags.tag_values,
                }),
                Err(e) => FileOutcome::Problem(problems::from_read_error(&candidate.path, &e)),
            }
        } else {
            FileOutcome::Unchanged
        };
    }

    // Stage 3: read headers. A known file that cannot be read keeps its track, as a problem.
    let read = match tags::read(&candidate.path) {
        Ok(r) => r,
        Err(e) => {
            let problem = problems::from_read_error(&candidate.path, &e);
            return FileOutcome::Problem(problem);
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
    let lyrics_sidecar = if read.tags.lyrics.is_none() { stamped.read_lyrics() } else { None };

    let title = read.tags.title.clone().unwrap_or_else(|| filename_title(&candidate.path));
    let (artist, album_artist) = artist_sort_keys(&read.tags);
    let sort = SortKeys {
        title: sort_key(&title, read.tags.title_sort.as_deref()),
        artist,
        album_artist,
        album: sort_key(read.tags.album.as_deref().unwrap_or(""), read.tags.album_sort.as_deref()),
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
        sidecar_stamp: stamped.stamp,
        sort,
    };

    match identity {
        Identity::New => FileOutcome::Added(Box::new(record)),
        Identity::Updated { track_id } => {
            FileOutcome::Updated(Box::new(TrackRecord { id: Some(track_id), ..record }))
        }
    }
}

/// The sort keys the file's first artist and first album artist vote for their artists' sort
/// keys with. A file votes at most once for an artist: where its first album artist is its first
/// artist, as it always is when the album artist falls back to the primary artist, the one vote
/// is as artist, with `ARTISTSORT` else `ALBUMARTISTSORT`. Otherwise the artist votes with
/// `ARTISTSORT` and the album artist with `ALBUMARTISTSORT`. An album artist that doesn't vote,
/// for want of a tag or being the same artist, or the unknown artist, gets the empty key, which
/// casts no vote, so only a tag can outvote an artist's own.
fn artist_sort_keys(tags: &TagSet) -> (String, String) {
    let first = tags.artists.first().map_or("", String::as_str);
    let (own, album_tag) = (tags.artist_sort.as_deref(), tags.album_artist_sort.as_deref());
    match tags.album_artists_or_primary().first().map(String::as_str) {
        Some(name) if name_key(name) == name_key(first) => {
            (sort_key(first, own.or(album_tag)), String::new())
        }
        Some(name) => {
            (sort_key(first, own), album_tag.map_or_else(String::new, |t| sort_key(name, Some(t))))
        }
        None => (sort_key(first, own), String::new()),
    }
}
