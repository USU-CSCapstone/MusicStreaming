//! A track record: everything a scan learns about one file, which the store persists.

use std::path::{Path, PathBuf};

use crate::audio::AudioProperties;
use crate::image::ImageInfo;
use crate::sidecar::{Biography, SidecarImage, SidecarStamp};
use crate::{Lyrics, TagSet};

/// A track's id, as the store assigns it.
pub type TrackId = i64;

/// Where a track's artwork was found (`requirements/scanning.md` §3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtworkSource {
    /// Inside the track file itself.
    Embedded,
    /// A `cover.*` sidecar at this path.
    Sidecar(PathBuf),
}

/// Artwork found for a track, with what the store needs for an `images` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artwork {
    pub source: ArtworkSource,
    pub info: ImageInfo,
}

/// Sort keys computed by the core at scan time (`design/general.md` §3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SortKeys {
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
}

/// Everything the scanner knows about one file, ready to persist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackRecord {
    /// `Some` when updating an existing track, `None` for a new one.
    pub id: Option<TrackId>,
    /// The library root this file lives under.
    pub root: PathBuf,
    /// Absolute path.
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: u64,
    pub tags: TagSet,
    pub properties: AudioProperties,
    /// Embedded art wins over a sidecar (`requirements/scanning.md` §3.1).
    pub artwork: Option<Artwork>,
    pub artist_image: Option<SidecarImage>,
    /// An `artist.txt` sidecar.
    pub artist_biography: Option<Biography>,
    /// Sidecar lyrics, only when the file has no embedded lyrics.
    pub lyrics_sidecar: Option<Lyrics>,
    /// The stamp of the sidecars it resolved, its lyrics file included, which the next scan
    /// compares.
    pub sidecar_stamp: SidecarStamp,
    pub sort: SortKeys,
    /// The generation of tag types its tags were read with ([`crate::TagTypes::generation`]).
    pub tags_generation: u32,
}

impl TrackRecord {
    /// The display title: the tag, or the filename stem
    /// (`requirements/scanning.md` §2).
    pub fn display_title(&self) -> String {
        self.tags.title.clone().unwrap_or_else(|| filename_title(&self.path))
    }

    /// Whichever lyrics apply: embedded first, then sidecar.
    pub fn lyrics(&self) -> Option<&Lyrics> {
        self.tags.lyrics.as_ref().or(self.lyrics_sidecar.as_ref())
    }
}

/// The title for a file with no title tag: its filename without the extension, or
/// "Untitled" when the path has no filename (`requirements/scanning.md` §2).
pub fn filename_title(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned())
}
