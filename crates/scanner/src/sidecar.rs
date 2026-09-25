//! Stage 5: sidecar resolution (`requirements/scanning.md` §3,
//! `design/scanning.md` §5).
//!
//! The filenames are a public contract plugins write against. They are
//! constants here and nowhere else. Resolution is per directory and memoized
//! for the life of a scan; the walk never escapes the library root. Sidecar
//! files are read here, once per directory: image headers and hashes for the
//! `images` table, biography text, and lyric files per track.

use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use jewelcase_core::Lyrics;

use crate::image;
use crate::types::{Biography, SidecarImage, SidecarLyrics};

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

/// What resolved for one directory, after the upward walk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sidecars {
    pub cover: Option<SidecarImage>,
    pub artist_image: Option<SidecarImage>,
    pub biography: Option<Biography>,
}

impl Sidecars {
    fn is_complete(&self) -> bool {
        self.cover.is_some() && self.artist_image.is_some() && self.biography.is_some()
    }
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
    /// Per directory: what is *in* it (not inherited).
    local: HashMap<PathBuf, Sidecars>,
    /// Per directory: what resolves for it after the walk.
    resolved: HashMap<PathBuf, Sidecars>,
}

impl SidecarResolver {
    pub fn new(root: impl Into<PathBuf>) -> SidecarResolver {
        SidecarResolver {
            root: root.into(),
            local: HashMap::new(),
            resolved: HashMap::new(),
        }
    }

    /// Use a listing discovery already made, so the directory is not read
    /// twice.
    pub fn offer_listing(&mut self, dir: &Path, files: &BTreeSet<OsString>) {
        if !self.local.contains_key(dir) {
            let local = local_from_names(dir, files.iter().map(|f| f.as_os_str()));
            self.local.insert(dir.to_path_buf(), local);
        }
    }

    /// Resolve for `dir`, walking up to the root. Nearest match wins
    /// (`requirements/scanning.md` §3.5).
    pub fn resolve(&mut self, dir: &Path) -> Sidecars {
        if let Some(r) = self.resolved.get(dir) {
            return r.clone();
        }
        let mut result = self.local_for(dir);
        if !result.is_complete()
            && dir != self.root
            && dir.starts_with(&self.root)
            && let Some(parent) = dir.parent()
        {
            let inherited = self.resolve(parent);
            result.cover = result.cover.or(inherited.cover);
            result.artist_image = result.artist_image.or(inherited.artist_image);
            result.biography = result.biography.or(inherited.biography);
        }
        self.resolved.insert(dir.to_path_buf(), result.clone());
        result
    }

    fn local_for(&mut self, dir: &Path) -> Sidecars {
        if let Some(l) = self.local.get(dir) {
            return l.clone();
        }
        // A parent the walk did not list (it lists top-down, so this is
        // ancestors of a folder-scoped scan): read it once.
        let local = match fs::read_dir(dir) {
            Ok(entries) => {
                let names: Vec<OsString> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name())
                    .collect();
                local_from_names(dir, names.iter().map(|n| n.as_os_str()))
            }
            Err(_) => Sidecars::default(),
        };
        self.local.insert(dir.to_path_buf(), local.clone());
        local
    }

    /// Lyrics: same directory, same stem, no walk (`requirements/scanning.md` §3.4).
    /// Returns the path, without reading it.
    pub fn lyrics_path_for(audio: &Path, files: &BTreeSet<OsString>) -> Option<PathBuf> {
        let stem = audio.file_stem()?;
        for ext in LYRICS_EXTENSIONS {
            let mut name = stem.to_os_string();
            name.push(".");
            name.push(ext);
            if files.contains(&name) {
                return Some(audio.with_file_name(name));
            }
            // Case-insensitive fallback for `.LRC`.
            if let Some(found) = files.iter().find(|f| f.eq_ignore_ascii_case(&name)) {
                return Some(audio.with_file_name(found));
            }
        }
        None
    }

    /// Lyrics for a track, read. An unreadable or empty file is no lyrics.
    pub fn lyrics_for(audio: &Path, files: &BTreeSet<OsString>) -> Option<SidecarLyrics> {
        let path = Self::lyrics_path_for(audio, files)?;
        let text = read_text(&path)?;
        Some(SidecarLyrics {
            path,
            lyrics: Lyrics::from_text(text),
        })
    }
}

fn read_bytes(path: &Path) -> Option<Vec<u8>> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_SIDECAR_BYTES {
        return None;
    }
    fs::read(path).ok()
}

fn read_text(path: &Path) -> Option<String> {
    let bytes = read_bytes(path)?;
    let text = String::from_utf8_lossy(&bytes).trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}

fn read_image(path: &Path) -> Option<SidecarImage> {
    let bytes = read_bytes(path)?;
    let info = image::describe(&bytes)?;
    Some(SidecarImage {
        path: path.to_path_buf(),
        info,
    })
}

fn local_from_names<'a>(dir: &Path, names: impl Iterator<Item = &'a OsStr>) -> Sidecars {
    let mut out = Sidecars::default();
    // Prefer extensions in IMAGE_EXTENSIONS order when several covers exist,
    // so the choice is deterministic across filesystems.
    let mut cover: Option<(usize, PathBuf)> = None;
    let mut artist: Option<(usize, PathBuf)> = None;
    for name in names {
        let Some(s) = name.to_str() else { continue };
        let lower = s.to_ascii_lowercase();
        if lower == BIOGRAPHY_NAME {
            let path = dir.join(name);
            out.biography = read_text(&path).map(|text| Biography { path, text });
            continue;
        }
        let Some((stem, ext)) = lower.rsplit_once('.') else {
            continue;
        };
        let Some(rank) = IMAGE_EXTENSIONS.iter().position(|e| *e == ext) else {
            continue;
        };
        if stem == COVER_STEM && cover.as_ref().is_none_or(|(r, _)| rank < *r) {
            cover = Some((rank, dir.join(name)));
        } else if stem == ARTIST_STEM && artist.as_ref().is_none_or(|(r, _)| rank < *r) {
            artist = Some((rank, dir.join(name)));
        }
    }
    out.cover = cover.and_then(|(_, p)| read_image(&p));
    out.artist_image = artist.and_then(|(_, p)| read_image(&p));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1x1 PNG.
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn touch(p: &Path, bytes: &[u8]) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    #[test]
    fn nearest_wins_and_walk_stops_at_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        touch(&root.join("cover.png"), PNG);
        touch(&root.join("Artist/artist.jpg"), PNG);
        touch(&root.join("Artist/artist.txt"), b"A band.");
        touch(&root.join("Artist/Album/cover.jpg"), PNG);
        touch(&root.join("Artist/Other/01.flac"), b"x");
        // Above the root: must never be seen.
        touch(&tmp.path().join("artist.png"), PNG);

        let mut r = SidecarResolver::new(&root);
        let album = r.resolve(&root.join("Artist/Album"));
        assert_eq!(
            album.cover.unwrap().path,
            root.join("Artist/Album/cover.jpg")
        );
        assert_eq!(
            album.artist_image.unwrap().path,
            root.join("Artist/artist.jpg")
        );
        assert_eq!(album.biography.unwrap().text, "A band.");

        let other = r.resolve(&root.join("Artist/Other"));
        assert_eq!(other.cover.unwrap().path, root.join("cover.png"));

        let top = r.resolve(&root);
        assert_eq!(top.artist_image, None);
    }

    #[test]
    fn unreadable_image_is_not_artwork() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        touch(&root.join("A/cover.jpg"), b"not an image");
        let mut r = SidecarResolver::new(&root);
        assert_eq!(r.resolve(&root.join("A")).cover, None);
    }

    #[test]
    fn lyrics_match_stem_only_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        touch(&dir.join("01 - Song.lrc"), b"[00:01.00]hi");
        touch(&dir.join("02 - Other.TXT"), b"plain words");
        let files: BTreeSet<OsString> = [
            "01 - Song.flac",
            "01 - Song.lrc",
            "02 - Other.flac",
            "02 - Other.TXT",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let a = SidecarResolver::lyrics_for(&dir.join("01 - Song.flac"), &files).unwrap();
        assert!(a.lyrics.synced);
        assert_eq!(a.path, dir.join("01 - Song.lrc"));
        let b = SidecarResolver::lyrics_for(&dir.join("02 - Other.flac"), &files).unwrap();
        assert!(!b.lyrics.synced);
        assert_eq!(
            SidecarResolver::lyrics_for(&dir.join("03.flac"), &files),
            None
        );
    }

    #[test]
    fn sidecar_names() {
        assert!(is_sidecar_name(OsStr::new("cover.jpg")));
        assert!(is_sidecar_name(OsStr::new("Cover.PNG")));
        assert!(is_sidecar_name(OsStr::new("artist.txt")));
        assert!(!is_sidecar_name(OsStr::new("cover.txt")));
        assert!(!is_sidecar_name(OsStr::new("front.jpg")));
    }
}
