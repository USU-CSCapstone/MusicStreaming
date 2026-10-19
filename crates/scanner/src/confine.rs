//! Symlinks are never followed (`requirements/scanning.md` §5): a link could lead outside the
//! library's roots. The walk skips every link it lists; a scope, and every file the server
//! serves or decodes, is refused through [`without_links`] when a link sits anywhere below its
//! root. A file is decoded only if it is a regular one ([`not_a_file`]).

use std::path::{Path, PathBuf};
use std::{error, fmt, io};

/// The reason inside an error from [`not_followed`] or [`not_a_file`], so a refusal can be told
/// from a file that is not there.
#[derive(Debug)]
struct Refused(&'static str);

impl fmt::Display for Refused {
    /// The problem detail a refused path is recorded with.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl error::Error for Refused {}

/// Why a symlink, or a path through one, is refused. `NotFound`: to the library, nothing is
/// there.
pub fn not_followed() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, Refused("symlinks are not followed"))
}

/// Why anything but a regular file is refused for decoding: opening a pipe or a device, or
/// decoding it, could block forever.
pub fn not_a_file() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, Refused("not a regular file"))
}

/// Whether `error` is a refusal from [`not_followed`] or [`not_a_file`], which no retry changes.
pub fn is_refused(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| inner.is::<Refused>())
}

/// The real path of `path`, which lies under `root`, refused unless no symlink sits anywhere
/// between `root` and it. The root itself may be reached through a link. A link that leads
/// nowhere is refused too, not reported missing.
pub fn without_links(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let real = path
        .canonicalize()
        .map_err(|error| if path.is_symlink() { not_followed() } else { error })?;
    let rel = path.strip_prefix(root).map_err(|_| not_followed())?;
    if real == root.canonicalize()?.join(rel) { Ok(real) } else { Err(not_followed()) }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn a_link_below_the_root_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let (real, music) = (tmp.path().join("real"), tmp.path().join("music"));
        fs::create_dir_all(real.join("A")).unwrap();
        fs::write(real.join("A/01.flac"), b"x").unwrap();
        // The root reached through a link, and links to a file and a folder inside it.
        symlink(&real, &music).unwrap();
        symlink(real.join("A/01.flac"), real.join("A/02.flac")).unwrap();
        symlink(real.join("A"), real.join("B")).unwrap();

        let kept = without_links(&music, &music.join("A/01.flac")).unwrap();
        assert_eq!(kept, real.canonicalize().unwrap().join("A/01.flac"));
        assert!(without_links(&music, &music).is_ok());
        symlink(real.join("gone.flac"), real.join("A/03.flac")).unwrap();
        for refused in ["A/02.flac", "A/03.flac", "B", "B/01.flac"] {
            let error = without_links(&music, &music.join(refused)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::NotFound, "{refused}");
            assert!(is_refused(&error), "{refused}");
        }
        let missing = without_links(&music, &music.join("A/04.flac")).unwrap_err();
        assert!(!is_refused(&missing), "a missing file is not a link");
        assert!(without_links(&music, &tmp.path().join("real/A/01.flac")).is_err(), "not below");
    }
}
