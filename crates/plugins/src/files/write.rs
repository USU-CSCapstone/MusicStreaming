//! Writing a whole file: staged beside its target under a name no other call uses, then
//! renamed or linked into place, so it appears whole or not at all.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use cap_std::fs::{File, OpenOptions};

use super::{MAX_WRITE, Placed, describe, open, place_new, reason, relative, split};
use crate::host::{Error, WriteMode};

/// Writes `contents` to `path`, as `mode` says: `Create` makes a new file, making its folders,
/// and fails if one is there, even one that appears during the call; `Replace` puts it in place
/// of the file there, and fails unless a file is there. Returns the path written on this host.
pub fn write(root: &Path, path: &str, contents: &[u8], mode: WriteMode) -> Result<PathBuf, Error> {
    if contents.len() > MAX_WRITE {
        let refused = format!("a file written at once is at most {} MB", MAX_WRITE >> 20);
        return Err(Error::Refused(refused));
    }
    let relative = relative(path)?;
    let (folder, name) = split(relative);
    let root_dir = open(root)?;
    if mode == WriteMode::Create {
        root_dir.create_dir_all(folder).map_err(|e| describe(path, e))?;
    }
    let folder = root_dir.open_dir(folder).map_err(|e| describe(path, e))?;
    if mode == WriteMode::Replace
        && !folder.metadata(name).map_err(|e| describe(path, e))?.is_file()
    {
        return Err(Error::Failed(format!("{path} is not a file")));
    }
    // Named apart from the target, so a target name of any allowed length can be staged.
    let staged = format!(".{:016x}.part", fastrand::u64(..));
    let file = folder
        .open_with(&staged, OpenOptions::new().write(true).create_new(true))
        .map_err(|e| describe(path, e))?;
    // From here the staged file is this call's own, so it is removed unless renamed into place.
    let placed = stage(file, contents).and_then(|()| match mode {
        WriteMode::Create => place_new(&folder, OsStr::new(&staged), &folder, name),
        WriteMode::Replace => folder.rename(&staged, &folder, name).map(|()| Placed::Moved),
    });
    settle(path, &staged, placed, || folder.remove_file(&staged))?;
    Ok(root.join(relative))
}

/// What became of writing `path` once its staged copy `staged` was `placed`: unless it was
/// renamed into place, the staged copy is removed with `remove`, and a removal that fails is
/// reported, saying whether the file was written.
pub(super) fn settle(
    path: &str,
    staged: &str,
    placed: io::Result<Placed>,
    remove: impl FnOnce() -> io::Result<()>,
) -> Result<(), Error> {
    if let Ok(Placed::Moved) = placed {
        return Ok(());
    }
    let left =
        |e: io::Error| format!("its staged copy {staged} could not be removed: {}", e.kind());
    match (placed, remove()) {
        (Ok(_), Ok(())) => Ok(()),
        (Ok(_), Err(e)) => Err(Error::Failed(format!("{path} was written, but {}", left(e)))),
        (Err(error), Ok(())) => Err(describe(path, error)),
        (Err(error), Err(e)) => {
            Err(Error::Failed(format!("{}, and {}", reason(path, &error), left(e))))
        }
    }
}

/// Writes `contents` to the staged file and waits for them to reach the disk.
fn stage(mut file: File, contents: &[u8]) -> io::Result<()> {
    file.write_all(contents)?;
    file.sync_all()
}
