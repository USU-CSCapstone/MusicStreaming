//! Putting a file or folder at a new name without replacing anything there, in one step no
//! other change can come between: `renameat2` with `RENAME_NOREPLACE`, or, on a filesystem
//! without it (NFS, Ceph, 9p, and FUSE ones that lack it), a hard link for a file. A folder
//! cannot be linked, so there it is renamed plainly, which can replace an empty folder.

use std::ffi::OsStr;
use std::io::{self, ErrorKind};

use cap_std::fs::Dir;
use rustix::fs::{AtFlags, RenameFlags, linkat, renameat, renameat_with};
use rustix::io::Errno;

/// How [`place_new`] put a file at its new name.
#[derive(Debug, PartialEq)]
pub(super) enum Placed {
    /// Renamed there: the old name is gone.
    Moved,
    /// Linked there: the old name is still there, for the caller to remove.
    Linked,
}

/// Puts the file `from` in `from_dir` at `to` in `to_dir`, both single names, failing with
/// `AlreadyExists` if `to` is there. Renames it where the filesystem can do that without
/// replacing, and links it otherwise.
pub(super) fn place_new(
    from_dir: &Dir,
    from: &OsStr,
    to_dir: &Dir,
    to: &OsStr,
) -> io::Result<Placed> {
    match renameat_with(from_dir, from, to_dir, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(Placed::Moved),
        Err(Errno::INVAL) => link_new(from_dir, from, to_dir, to).map(|()| Placed::Linked),
        Err(e) => Err(e.into()),
    }
}

/// Links the file `from` in `from_dir` at `to` in `to_dir`, failing with `AlreadyExists` if `to`
/// is there. It fails with the filesystem's error where hard links are not made, and, under
/// `fs.protected_hardlinks`, for a file the server's user neither owns nor can read and write.
pub(super) fn link_new(from_dir: &Dir, from: &OsStr, to_dir: &Dir, to: &OsStr) -> io::Result<()> {
    Ok(linkat(from_dir, from, to_dir, to, AtFlags::empty())?)
}

/// Renames the folder `from` in `from_dir` to `to` in `to_dir`, failing with `AlreadyExists` if
/// `to` is there. A folder cannot be linked, so on a filesystem without `RENAME_NOREPLACE` it is
/// renamed plainly, which still fails over a file or a folder with anything in it, but replaces
/// an empty folder. Either way, what is at `to` fails it as `AlreadyExists`, so the failure
/// names `to`.
pub(super) fn move_folder(
    from_dir: &Dir,
    from: &OsStr,
    to_dir: &Dir,
    to: &OsStr,
) -> io::Result<()> {
    match renameat_with(from_dir, from, to_dir, to, RenameFlags::NOREPLACE) {
        // A plain rename answers a file or a folder with anything in it at `to` this way.
        Err(Errno::INVAL) => renameat(from_dir, from, to_dir, to).map_err(|e| match e {
            Errno::NOTEMPTY | Errno::NOTDIR => ErrorKind::AlreadyExists.into(),
            e => e.into(),
        }),
        renamed => Ok(renamed?),
    }
}
