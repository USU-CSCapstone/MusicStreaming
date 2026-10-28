//! What the suite reads back from a store.

use std::collections::BTreeMap;
use std::path::PathBuf;

use jewelcase_core::Format;

use crate::library::LibraryId;
use crate::problems::Problem;
use crate::scan::Scan;
use crate::store::{AnalysisStore, PlaceholderStore, Store};
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
    /// Its album's named artists, in order; empty for the unknown artist.
    pub album_artists: Vec<String>,
    pub has_artwork: bool,
    pub has_biography: bool,
    /// Whether it has lyrics. The suite's files embed none, so any it has are a lyrics file's.
    pub has_lyrics: bool,
    pub sort_title: String,
    pub sort_artist: String,
    pub format: Format,
    pub channels: Option<u8>,
    pub sample_rate: Option<u32>,
    pub analyzed: bool,
    /// Whether its analysis measured a loudness.
    pub measured: bool,
    /// Its tags by type, each as the file spells it, in that order.
    pub tags: BTreeMap<String, Vec<String>>,
}

/// What a store must expose for the suite to check it.
pub trait InspectStore: Store + AnalysisStore + PlaceholderStore {
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
    /// Marks every track of `library` read before any tag type, as a release adding a type finds
    /// them ([`crate::tags_generation`]).
    fn age_tag_reads(&self, library: LibraryId);
}
