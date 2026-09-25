//! Stage 5: sidecar resolution (`requirements/scanning.md` §3,
//! `design/scanning.md` §5).
//!
//! The filenames are a public contract plugins write against. They are
//! constants here and nowhere else. Resolution is per directory and memoized
//! for the life of a scan; the walk never escapes the library root.

use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

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

/// What resolved for one directory, after the upward walk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sidecars {
    pub cover: Option<PathBuf>,
    pub artist_image: Option<PathBuf>,
    pub biography: Option<PathBuf>,
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
    pub fn lyrics_for(audio: &Path, files: &BTreeSet<OsString>) -> Option<PathBuf> {
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
}

impl Sidecars {
    fn is_complete(&self) -> bool {
        self.cover.is_some() && self.artist_image.is_some() && self.biography.is_some()
    }
}

fn local_from_names<'a>(dir: &Path, names: impl Iterator<Item = &'a OsStr>) -> Sidecars {
    let mut out = Sidecars::default();
    // Prefer extensions in IMAGE_EXTENSIONS order when several covers exist,
    // so the choice is deterministic across filesystems.
    let mut cover_rank = usize::MAX;
    let mut artist_rank = usize::MAX;
    for name in names {
        let Some(s) = name.to_str() else { continue };
        let lower = s.to_ascii_lowercase();
        if lower == BIOGRAPHY_NAME {
            out.biography = Some(dir.join(name));
            continue;
        }
        let Some((stem, ext)) = lower.rsplit_once('.') else {
            continue;
        };
        let Some(rank) = IMAGE_EXTENSIONS.iter().position(|e| *e == ext) else {
            continue;
        };
        if stem == COVER_STEM && rank < cover_rank {
            cover_rank = rank;
            out.cover = Some(dir.join(name));
        } else if stem == ARTIST_STEM && rank < artist_rank {
            artist_rank = rank;
            out.artist_image = Some(dir.join(name));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    #[test]
    fn nearest_wins_and_walk_stops_at_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        touch(&root.join("cover.png"));
        touch(&root.join("Artist/artist.jpg"));
        touch(&root.join("Artist/artist.txt"));
        touch(&root.join("Artist/Album/cover.jpg"));
        touch(&root.join("Artist/Other/01.flac"));
        // Above the root: must never be seen.
        touch(&tmp.path().join("artist.png"));

        let mut r = SidecarResolver::new(&root);
        let album = r.resolve(&root.join("Artist/Album"));
        assert_eq!(album.cover, Some(root.join("Artist/Album/cover.jpg")));
        assert_eq!(album.artist_image, Some(root.join("Artist/artist.jpg")));
        assert_eq!(album.biography, Some(root.join("Artist/artist.txt")));

        let other = r.resolve(&root.join("Artist/Other"));
        assert_eq!(other.cover, Some(root.join("cover.png")));

        let top = r.resolve(&root);
        assert_eq!(top.artist_image, None);
    }

    #[test]
    fn lyrics_match_stem_only_in_place() {
        let files: BTreeSet<OsString> = [
            "01 - Song.flac",
            "01 - Song.lrc",
            "02 - Other.flac",
            "02 - Other.TXT",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        assert_eq!(
            SidecarResolver::lyrics_for(Path::new("/m/01 - Song.flac"), &files),
            Some(PathBuf::from("/m/01 - Song.lrc"))
        );
        assert_eq!(
            SidecarResolver::lyrics_for(Path::new("/m/02 - Other.flac"), &files),
            Some(PathBuf::from("/m/02 - Other.TXT"))
        );
        assert_eq!(
            SidecarResolver::lyrics_for(Path::new("/m/03.flac"), &files),
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
