//! Writing a whole file: `Create` opens its name as a new file; `Replace` stages a copy beside
//! its target and renames it over.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::Path;

use cap_std::fs::{Dir, File, OpenOptions};

use super::{MAX_WRITE, describe, open, relative, split};
use crate::host::{Error, WriteMode};

/// Writes `contents` to `path`, as `mode` says: `Create` makes a new file, making its folders,
/// and fails if anything is there, a link included, even one that appears during the call;
/// `Replace` puts it in place of the file there, and fails unless a file is there. A `Replace`
/// appears whole or not at all; a `Create` that fails part way is removed, but one cut short by
/// a crash can leave a truncated file.
pub fn write(root: &Path, path: &str, contents: &[u8], mode: WriteMode) -> Result<(), Error> {
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
    match mode {
        WriteMode::Create => {
            let mut file = new_file(&folder, name).map_err(|e| describe(path, e))?;
            file.write_all(contents).and_then(|()| file.sync_all()).map_err(|e| {
                // The file is this call's own, so a failed write leaves nothing behind.
                let _ = folder.remove_file(name);
                describe(path, e)
            })
        }
        WriteMode::Replace => replace(&folder, path, name, contents),
    }
}

/// Replaces the file `name` in `folder` with `contents`, staged beside it first. Fails unless a
/// file is there; `path` is how the failure names it.
fn replace(folder: &Dir, path: &str, name: &OsStr, contents: &[u8]) -> Result<(), Error> {
    if !folder.metadata(name).map_err(|e| describe(path, e))?.is_file() {
        return Err(Error::Failed(format!("{path} is not a file")));
    }
    // Named apart from the target, so a target name of any allowed length can be staged.
    let staged = format!(".{:016x}.part", fastrand::u64(..));
    let mut file = new_file(folder, &staged).map_err(|e| describe(path, e))?;
    let renamed = file
        .write_all(contents)
        .and_then(|()| file.sync_all())
        .and_then(|()| folder.rename(&staged, folder, name));
    renamed.map_err(|e: io::Error| {
        let _ = folder.remove_file(&staged);
        describe(path, e)
    })
}

/// Opens `name` in `folder` for writing as a file that did not exist. It fails on anything
/// already there, and never follows a link there, even a dangling one.
fn new_file(folder: &Dir, name: impl AsRef<Path>) -> io::Result<File> {
    folder.open_with(name, OpenOptions::new().write(true).create_new(true))
}
