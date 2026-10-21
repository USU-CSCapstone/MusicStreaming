//! What the suite reads back from a store, and how the memory store answers it.

use std::path::PathBuf;

use jewelcase_core::Format;

use crate::library::LibraryId;
use crate::problems::Problem;
use crate::scan::Scan;
use crate::store::{FeedEntry, MemoryStore, Store};
use crate::track::TrackId;

/// A store-agnostic view of one indexed track, for assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectTrack {
    pub id: TrackId,
    pub path: PathBuf,
    pub missing: bool,
    /// The displayed title: the tag, or the filename stem.
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub has_artwork: bool,
    pub has_biography: bool,
    pub has_sidecar_lyrics: bool,
    pub sort_title: String,
    pub sort_artist: String,
    pub format: Format,
    pub channels: Option<u8>,
    pub sample_rate: Option<u32>,
    pub analyzed: bool,
    /// Whether its analysis measured a loudness.
    pub measured: bool,
}

/// What a store must expose for the suite to check it.
pub trait InspectStore: Store {
    /// Every track of `library`, missing or not.
    fn inspect_tracks(&self, library: LibraryId) -> Vec<InspectTrack>;
    /// Every problem recorded for `library`.
    fn inspect_problems(&self, library: LibraryId) -> Vec<Problem>;
    /// Every scan of `library`.
    fn inspect_scans(&self, library: LibraryId) -> Vec<Scan>;
    /// The change feed's head: the position of its latest entry, or zero when it is
    /// empty. Every entry written raises it, so an unchanged head means nothing was
    /// written, even to a feed that keeps one entry per item.
    fn feed_head(&self, library: LibraryId) -> i64;
    /// The tracks named by feed entries after position `head`, oldest first.
    fn feed_tracks_after(&self, library: LibraryId, head: i64) -> Vec<TrackId>;
    /// The placeholder stored for the image with content `hash`.
    fn inspect_placeholder(&self, library: LibraryId, hash: &[u8]) -> Option<Vec<u8>>;
}

impl InspectStore for MemoryStore {
    fn inspect_tracks(&self, library: LibraryId) -> Vec<InspectTrack> {
        self.tracks(library)
            .into_iter()
            .map(|t| InspectTrack {
                id: t.record.id.unwrap_or(0),
                path: t.record.path.clone(),
                missing: t.missing,
                title: t.record.display_title(),
                artists: t.record.tags.artists.clone(),
                album: t.record.tags.album.clone(),
                has_artwork: t.record.artwork.is_some(),
                has_biography: t.record.artist_biography.is_some(),
                has_sidecar_lyrics: t.record.lyrics_sidecar.is_some(),
                sort_title: t.record.sort.title.clone(),
                sort_artist: t.record.sort.artist.clone(),
                format: t.record.properties.format,
                channels: t.record.properties.channels,
                sample_rate: t.record.properties.sample_rate,
                analyzed: t.analysis.is_some(),
                measured: t.analysis.is_some_and(|a| a.integrated_lufs.is_some()),
            })
            .collect()
    }

    fn inspect_placeholder(&self, library: LibraryId, hash: &[u8]) -> Option<Vec<u8>> {
        self.placeholder(library, hash)
    }

    fn inspect_problems(&self, library: LibraryId) -> Vec<Problem> {
        self.problems(library)
    }

    fn inspect_scans(&self, library: LibraryId) -> Vec<Scan> {
        self.scans(library)
    }

    fn feed_head(&self, library: LibraryId) -> i64 {
        // The memory feed only appends, so its length is its head.
        i64::try_from(self.feed(library).len()).unwrap()
    }

    fn feed_tracks_after(&self, library: LibraryId, head: i64) -> Vec<TrackId> {
        self.feed(library)
            .into_iter()
            .skip(usize::try_from(head).unwrap())
            .map(|entry| match entry {
                FeedEntry::Added(id)
                | FeedEntry::Updated(id)
                | FeedEntry::Missing(id)
                | FeedEntry::Returned(id) => id,
            })
            .collect()
    }
}
