//! Stage 5: sidecar resolution (`requirements/scanning.md` §3,
//! `design/scanning.md` §5).
//!
//! The filenames are a public contract plugins write against. They are
//! constants here and nowhere else. Resolution is per directory and memoized
//! for the life of a scan; the upward walk stops at the scan's root, and a
//! symlinked sidecar is never read ([`crate::confine`]). It has two steps.
//! Locating settles which file of each kind applies to a directory from
//! listings and the sizes and modified times they carry, which is all a
//! track's [stamp](SidecarResolver::stamp) needs. Reading opens what was
//! located, once per file per scan, and only a file past stage 2 asks for it,
//! so a directory of unchanged files opens none.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::GlobSet;
use sha2::{Digest, Sha256};

use self::locate::{Found, Located, LyricsNames};
use crate::discover::{FileStat, excluded};
use crate::image::ImageInfo;
use crate::{Lyrics, image};

mod locate;

/// `cover.*` — album art (`requirements/scanning.md` §3.1).
pub const COVER_STEM: &str = "cover";
/// `artist.*` — artist image (`requirements/scanning.md` §3.2).
pub const ARTIST_STEM: &str = "artist";
/// `artist.txt` — artist biography (`requirements/scanning.md` §3.3).
pub const BIOGRAPHY_NAME: &str = "artist.txt";

/// Image extensions a `cover.*` or `artist.*` may carry.
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "avif"];
/// Lyrics extensions, synchronized first (`requirements/scanning.md` §3.4).
pub const LYRICS_EXTENSIONS: &[&str] = &["lrc", "txt"];

/// Largest sidecar file read whole. Covers are rarely over a few megabytes;
/// anything larger is ignored rather than hashed.
const MAX_SIDECAR_BYTES: u64 = 32 * 1024 * 1024;

/// A hash of the path, size and modified time of each sidecar file a track resolved
/// ([`SidecarResolver::stamp`]). Only equality means anything.
pub type SidecarStamp = i64;

/// An `artist.*` sidecar image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarImage {
    pub path: PathBuf,
    pub info: ImageInfo,
}

/// The text of an `artist.txt`, shared by every track that resolves it, with the SHA-256 of
/// the text, by which the store keeps one row per distinct biography.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Biography {
    pub text: Arc<str>,
    pub hash: [u8; 32],
}

impl Biography {
    /// `text`, with its hash.
    pub fn new(text: &str) -> Biography {
        Biography { text: Arc::from(text), hash: Sha256::digest(text).into() }
    }
}

/// What resolved for one directory, after the upward walk, read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sidecars {
    pub cover: Option<SidecarImage>,
    pub artist_image: Option<SidecarImage>,
    pub biography: Option<Biography>,
}

/// A track's sidecar stamp, with the lyrics file it hashed, so the file is read without being
/// looked up again.
pub struct Stamped {
    pub stamp: SidecarStamp,
    lyrics: Option<Found>,
}

impl Stamped {
    /// The lyrics in the file the stamp hashed, read. An unreadable or empty file is no lyrics.
    pub fn read_lyrics(&self) -> Option<Lyrics> {
        Lyrics::parse(&read_text(&self.lyrics.as_ref()?.path)?)
    }
}

/// The sidecars that apply to a directory after the walk, and the digest of them that each
/// stamp in it starts from.
struct Applied {
    located: Located,
    digest: [u8; 32],
}

/// Whether a filename is one of the sidecar names. The watcher uses this to
/// widen a change to the subtree (`design/scanning.md` §9).
pub fn is_sidecar_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower == BIOGRAPHY_NAME {
        return true;
    }
    let Some((stem, ext)) = lower.rsplit_once('.') else {
        return false;
    };
    (stem == COVER_STEM || stem == ARTIST_STEM) && IMAGE_EXTENSIONS.contains(&ext)
}

/// Resolver for one scan of one root.
pub struct SidecarResolver {
    root: PathBuf,
    /// The scan's excludes: a listed file they match is no sidecar, as in the walk's listings.
    excludes: GlobSet,
    /// Per listed directory not yet located: the sidecars in it (not inherited).
    local: HashMap<PathBuf, Located>,
    /// Per directory: the sidecars that apply to it after the walk.
    applied: HashMap<PathBuf, Applied>,
    /// The lyrics files of the directory whose tracks were last stamped.
    lyrics_names: Option<LyricsNames>,
    /// Per located image: what reading it gave.
    images: HashMap<PathBuf, Option<SidecarImage>>,
    /// Per located biography: what reading it gave.
    biographies: HashMap<PathBuf, Option<Biography>>,
}

impl SidecarResolver {
    /// A resolver for a scan of the root at `root` that applies `excludes`, with nothing located
    /// or read yet.
    pub fn new(root: impl Into<PathBuf>, excludes: GlobSet) -> SidecarResolver {
        SidecarResolver {
            root: root.into(),
            excludes,
            local: HashMap::new(),
            applied: HashMap::new(),
            lyrics_names: None,
            images: HashMap::new(),
            biographies: HashMap::new(),
        }
    }

    /// Use a listing discovery already made, so the directory is not listed or its sidecars
    /// stat'ed twice. Nothing is read until `dir` resolves.
    pub fn offer_listing(&mut self, dir: &Path, files: &BTreeMap<OsString, FileStat>) {
        if !self.local.contains_key(dir) {
            let names = files.keys().map(OsString::as_os_str);
            let local = Located::pick(dir, names, |name, path| Found::listed(path, files[name]));
            self.local.insert(dir.to_path_buf(), local);
        }
    }

    /// Resolve for `dir`: read what locating it found, each file once per scan. A located
    /// file that cannot be read, or is not an image, is no sidecar; the walk does not go on past
    /// it, so what a track resolved is decided by what its stamp hashes.
    pub fn resolve(&mut self, dir: &Path) -> Sidecars {
        let Located { cover, artist_image, biography } = self.locate(dir).located.clone();
        Sidecars {
            cover: cover.and_then(|found| self.image(found.path)),
            artist_image: artist_image.and_then(|found| self.image(found.path)),
            biography: biography.and_then(|found| {
                let read = |path: &PathBuf| read_text(path).map(|text| Biography::new(&text));
                self.biographies.entry(found.path).or_insert_with_key(read).clone()
            }),
        }
    }

    /// The stamp of the sidecars that apply to the track at `audio`, in `dir` whose listing is
    /// `files`: its cover, artist image and biography, and its lyrics file. Taken from listings
    /// alone, so an unchanged track's check opens nothing, and changed by a change to any of
    /// them, but not to a sidecar a nearer one overrides. A track with no lyrics file has its
    /// directory's digest, so only a track with one costs a hash.
    pub fn stamp(
        &mut self,
        dir: &Path,
        audio: &Path,
        files: &BTreeMap<OsString, FileStat>,
    ) -> Stamped {
        if !self.lyrics_names.as_ref().is_some_and(|names| names.dir() == dir) {
            self.lyrics_names = Some(LyricsNames::of(dir, files));
        }
        let lyrics = self.lyrics_names.as_ref().and_then(|names| names.find(audio, files));
        let digest = self.locate(dir).digest;
        let stamp = match &lyrics {
            None => head(&digest),
            Some(found) => {
                let mut hasher = Sha256::new();
                hasher.update(digest);
                found.hash_into(&mut hasher);
                head(&hasher.finalize())
            }
        };
        Stamped { stamp, lyrics }
    }

    /// The sidecars that apply to `dir`, walking up to the root, each directory located once.
    /// Nearest match wins (`requirements/scanning.md` §3.5).
    fn locate(&mut self, dir: &Path) -> &Applied {
        if !self.applied.contains_key(dir) {
            let mut located = self.local_for(dir);
            if !located.is_complete()
                && dir != self.root
                && dir.starts_with(&self.root)
                && let Some(parent) = dir.parent()
            {
                let inherited = &self.locate(parent).located;
                located.cover = located.cover.or_else(|| inherited.cover.clone());
                located.artist_image =
                    located.artist_image.or_else(|| inherited.artist_image.clone());
                located.biography = located.biography.or_else(|| inherited.biography.clone());
            }
            let digest = located.digest();
            self.applied.insert(dir.to_path_buf(), Applied { located, digest });
        }
        &self.applied[dir]
    }

    /// The sidecars in `dir`, taken from its offered listing. A directory the walk did not list
    /// (it lists top-down, so this is an ancestor of a folder-scoped or resumed scan) is listed
    /// here, its regular files the excludes leave in, as discovery lists, and only its sidecars
    /// are stat'ed. Called once per directory, since [`Self::locate`] memoizes what it returns.
    fn local_for(&mut self, dir: &Path) -> Located {
        if let Some(local) = self.local.remove(dir) {
            return local;
        }
        match fs::read_dir(dir) {
            Ok(entries) => {
                let names: Vec<OsString> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                    .filter(|e| !excluded(&self.root, &e.path(), &self.excludes))
                    .map(|e| e.file_name())
                    .collect();
                Located::pick(dir, names.iter().map(OsString::as_os_str), |_, path| Found::at(path))
            }
            Err(_) => Located::default(),
        }
    }

    /// The image at the located `path`, read the first time it is asked for.
    fn image(&mut self, path: PathBuf) -> Option<SidecarImage> {
        self.images.entry(path).or_insert_with_key(|p| read_image(p)).clone()
    }
}

/// The first eight bytes of `digest`, as a stamp.
fn head(digest: &[u8]) -> SidecarStamp {
    let mut head = [0; 8];
    head.copy_from_slice(&digest[..8]);
    SidecarStamp::from_le_bytes(head)
}

/// The whole file at `path`, if it is a non-empty regular file, not a symlink, no larger than
/// [`MAX_SIDECAR_BYTES`]: what was located may have changed since.
fn read_bytes(path: &Path) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_SIDECAR_BYTES {
        return None;
    }
    fs::read(path).ok()
}

/// The text of the file at `path` ([`read_bytes`]), without a leading byte order mark or
/// surrounding whitespace. `None` when the file can't be read or nothing is left.
fn read_text(path: &Path) -> Option<String> {
    let bytes = read_bytes(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_start_matches('\u{FEFF}').trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}

/// The image at `path`, if it can be read and described.
fn read_image(path: &Path) -> Option<SidecarImage> {
    let bytes = read_bytes(path)?;
    let info = image::describe(&bytes)?;
    Some(SidecarImage { path: path.to_path_buf(), info })
}

#[cfg(test)]
mod tests;
