//! A track record: everything a scan learns about one file, which the store persists.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::audio::AudioProperties;
use crate::image::ImageInfo;
use crate::scan::ScanId;
use crate::sidecar::{Biography, SidecarImage, SidecarLyrics};
use crate::{Lyrics, TagSet};

/// A track's id, as the store assigns it.
pub type TrackId = i64;

/// Where a track's artwork was found (`requirements/scanning.md` §3.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtworkSource {
    /// Inside the track file itself.
    Embedded,
    /// A `cover.*` sidecar at this path.
    Sidecar(PathBuf),
}

/// Artwork found for a track, with what the store needs for an `images` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artwork {
    pub source: ArtworkSource,
    pub info: ImageInfo,
}

/// Sort keys computed by the core at scan time (`design/general.md` §3).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortKeys {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
}

/// Everything the scanner knows about one file, ready to persist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub artist_biography: Option<Biography>,
    /// Sidecar lyrics, only when the file has no embedded lyrics.
    pub lyrics_sidecar: Option<SidecarLyrics>,
    pub sort: SortKeys,
    pub last_seen_scan: ScanId,
}

impl TrackRecord {
    /// The display title: the tag, or the filename stem
    /// (`requirements/scanning.md` §2).
    pub fn display_title(&self) -> String {
        self.tags.title.clone().unwrap_or_else(|| filename_title(&self.path))
    }

    /// Whichever lyrics apply: embedded first, then sidecar.
    pub fn lyrics(&self) -> Option<&Lyrics> {
        self.tags.lyrics.as_ref().or(self.lyrics_sidecar.as_ref().map(|l| &l.lyrics))
    }

    /// Path relative to the root, as the store keeps it.
    pub fn relative_path(&self) -> &Path {
        self.path.strip_prefix(&self.root).unwrap_or(&self.path)
    }
}

/// The title for a file with no title tag: its filename without the extension, or
/// "Untitled" when it has none (`requirements/scanning.md` §2).
pub fn filename_title(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".to_owned())
}
