//! Locating sidecars: which file of each kind applies, settled from listings and the sizes and
//! modified times they carry, without reading any.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{
    ARTIST_STEM, BIOGRAPHY_NAME, COVER_STEM, IMAGE_EXTENSIONS, LYRICS_EXTENSIONS, MAX_SIDECAR_BYTES,
};
use crate::clock::system_time_ms;
use crate::discover::FileStat;

/// A sidecar file found but not read: a regular file, not a symlink, neither empty nor larger
/// than [`MAX_SIDECAR_BYTES`].
#[derive(Debug, Clone)]
pub(super) struct Found {
    pub(super) path: PathBuf,
    stat: FileStat,
}

impl Found {
    /// The file at `path` as a listing found it, if a sidecar may be its size.
    pub(super) fn listed(path: PathBuf, stat: FileStat) -> Option<Found> {
        (1..=MAX_SIDECAR_BYTES).contains(&stat.size).then_some(Found { path, stat })
    }

    /// The file at `path`, stat'ed without following a symlink. One that cannot be stat'ed is
    /// absent, as a missing sidecar is.
    pub(super) fn at(path: PathBuf) -> Option<Found> {
        let meta = fs::symlink_metadata(&path).ok().filter(|meta| meta.is_file())?;
        let mtime_ms = meta.modified().map(system_time_ms).unwrap_or(0);
        Found::listed(path, FileStat { size: meta.len(), mtime_ms })
    }

    /// Feeds the path, size and modified time to `hasher`, each in a fixed or prefixed length,
    /// so no two files feed the same bytes.
    pub(super) fn hash_into(&self, hasher: &mut Sha256) {
        let path = self.path.as_os_str().as_encoded_bytes();
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path);
        hasher.update(self.stat.size.to_le_bytes());
        hasher.update(self.stat.mtime_ms.to_le_bytes());
    }
}

/// The sidecar file of each kind that applies to a directory, found but not read.
#[derive(Debug, Clone, Default)]
pub(super) struct Located {
    pub(super) cover: Option<Found>,
    pub(super) artist_image: Option<Found>,
    pub(super) biography: Option<Found>,
}

impl Located {
    /// Whether a sidecar of every kind is found, so the directory inherits nothing.
    pub(super) fn is_complete(&self) -> bool {
        self.cover.is_some() && self.artist_image.is_some() && self.biography.is_some()
    }

    /// The SHA-256 of the cover, artist image and biography, a marked absence for each one not
    /// found, which a track's stamp starts from.
    pub(super) fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for found in [&self.cover, &self.artist_image, &self.biography] {
            match found {
                None => hasher.update([0_u8]),
                Some(found) => {
                    hasher.update([1_u8]);
                    found.hash_into(&mut hasher);
                }
            }
        }
        hasher.finalize().into()
    }

    /// The sidecars `dir` holds itself, among the file `names` listed in it: of each kind, the
    /// first name `find` makes a [`Found`] of, given its name and path. Images are tried in
    /// [`IMAGE_EXTENSIONS`] order, and names of one rank, such as `artist.txt` and `ARTIST.TXT`,
    /// in byte order, so the choice is deterministic across filesystems and listing orders, and
    /// an empty `cover.jpg` gives way to a `cover.png` beside it.
    pub(super) fn pick<'a>(
        dir: &Path,
        names: impl Iterator<Item = &'a OsStr>,
        mut find: impl FnMut(&'a OsStr, PathBuf) -> Option<Found>,
    ) -> Located {
        // Each candidate with its rank: its extension's place for an image, 0 for a biography.
        let mut covers: Vec<(usize, &OsStr)> = Vec::new();
        let mut artists: Vec<(usize, &OsStr)> = Vec::new();
        let mut biographies: Vec<(usize, &OsStr)> = Vec::new();
        for name in names {
            let Some(s) = name.to_str() else { continue };
            let lower = s.to_ascii_lowercase();
            if lower == BIOGRAPHY_NAME {
                biographies.push((0, name));
                continue;
            }
            let Some((stem, ext)) = lower.rsplit_once('.') else {
                continue;
            };
            let Some(rank) = IMAGE_EXTENSIONS.iter().position(|e| *e == ext) else {
                continue;
            };
            if stem == COVER_STEM {
                covers.push((rank, name));
            } else if stem == ARTIST_STEM {
                artists.push((rank, name));
            }
        }
        let mut first = |mut ranked: Vec<(usize, &'a OsStr)>| {
            ranked.sort_unstable();
            ranked.into_iter().find_map(|(_, name)| find(name, dir.join(name)))
        };
        Located {
            cover: first(covers),
            artist_image: first(artists),
            biography: first(biographies),
        }
    }
}

/// The lyrics files one directory's listing names, by name folded to ASCII lowercase, so a track
/// whose lyrics file differs from it in case, such as `.LRC`, is matched without a pass over the
/// listing for each track.
pub(super) struct LyricsNames {
    dir: PathBuf,
    folded: HashMap<OsString, OsString>,
}

impl LyricsNames {
    /// The lyrics files among `files`, `dir`'s listing. Where two fold alike, the first in the
    /// listing's order is kept, so the choice is deterministic.
    pub(super) fn of(dir: &Path, files: &BTreeMap<OsString, FileStat>) -> LyricsNames {
        let mut folded = HashMap::new();
        for name in files.keys() {
            let lower = name.to_ascii_lowercase();
            let ext = Path::new(&lower).extension().and_then(OsStr::to_str);
            if ext.is_some_and(|ext| LYRICS_EXTENSIONS.contains(&ext)) {
                folded.entry(lower).or_insert_with(|| name.clone());
            }
        }
        LyricsNames { dir: dir.to_path_buf(), folded }
    }

    /// The directory whose listing this is.
    pub(super) fn dir(&self) -> &Path {
        &self.dir
    }

    /// The lyrics file for `audio` among `files`, this directory's listing: same stem, no walk
    /// (`requirements/scanning.md` §3.4). For each extension in turn the exact name, then one
    /// differing only in case; the first that names a file decides, whatever its size.
    pub(super) fn find(&self, audio: &Path, files: &BTreeMap<OsString, FileStat>) -> Option<Found> {
        let stem = audio.file_stem()?;
        for ext in LYRICS_EXTENSIONS {
            let mut name = stem.to_os_string();
            name.push(".");
            name.push(ext);
            let listed = files.get_key_value(&name).or_else(|| {
                let folded = self.folded.get(&name.to_ascii_lowercase())?;
                files.get_key_value(folded)
            });
            if let Some((name, stat)) = listed {
                return Found::listed(audio.with_file_name(name), *stat);
            }
        }
        None
    }
}
