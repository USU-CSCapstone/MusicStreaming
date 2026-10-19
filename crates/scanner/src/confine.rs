//! Symlinks are never followed (`requirements/scanning.md` §5): a link could lead outside the
//! library's roots. The walk skips every link it lists; a scope, and every file the server
//! serves or decodes, is refused through [`without_links`] when a link sits anywhere below its
//! root.

use std::io;
use std::path::{Path, PathBuf};

/// Why a symlink, or a path through one, is refused. `NotFound`: to the library, nothing is
/// there.
pub fn not_followed() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "symlinks are not followed")
}

/// The real path of `path`, which lies under `root`, refused unless no symlink sits anywhere
/// between `root` and it. The root itself may be reached through a link.
pub fn without_links(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let real = path.canonicalize()?;
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
        for refused in ["A/02.flac", "B", "B/01.flac"] {
            let error = without_links(&music, &music.join(refused)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::NotFound, "{refused}");
        }
        assert!(without_links(&music, &tmp.path().join("real/A/01.flac")).is_err(), "not below");
    }
}
