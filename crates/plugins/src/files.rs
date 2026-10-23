//! The library's folders, as plugins reach them (`requirements/plugins.md` §3): every path is
//! relative to one of the library's roots. One that is absolute or has `..` is refused as
//! written (see [`relative`]); every other is opened through a `cap_std` [`Dir`] on the root,
//! which refuses one that would leave it through a symlink however the folders change during
//! the call.
//!
//! Writes appear whole or not at all: a file is written beside its target under a name no
//! other call uses, then renamed or linked into place.
//!
//! A path the host will not use is `refused`; one the filesystem turns down is `failed`.

use std::collections::BinaryHeap;
use std::ffi::OsStr;
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use cap_std::ambient_authority;
use cap_std::fs::Dir;

use crate::host::{Entry, Error};
use place::{Placed, move_folder, place_new};

mod place;
mod write;

pub use write::write;

/// The largest file a plugin writes in one call. It is held in the plugin's memory whole.
const MAX_WRITE: usize = 32 << 20;
/// The most a plugin reads in one call.
pub const MAX_READ: u32 = 8 << 20;
/// The most entries one call to [`entries`] answers, as many as a page of the library.
const PAGE: usize = 500;

/// `path` as a relative path of one name or more, refused if it is absolute, has `..` or a leading
/// `.`, or holds a NUL or a backslash.
fn relative(path: &str) -> Result<&Path, Error> {
    if path.contains('\0') || path.contains('\\') {
        return Err(Error::Refused(format!("{path:?} is not a library path")));
    }
    let relative = Path::new(path);
    if relative.components().next().is_none() {
        return Err(Error::Refused("the path is empty".into()));
    }
    if !relative.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(Error::Refused(format!("{path:?} is not a path within the library")));
    }
    Ok(relative)
}

/// Where `path` is on this host, beneath `root`, refused as [`relative`] refuses it. Nothing on
/// disk is looked at, so this is where a call would reach, not where it may lead through a link.
pub fn resolve(root: &Path, path: &str) -> Result<PathBuf, Error> {
    Ok(root.join(relative(path)?))
}

/// The library's root, opened as the folder every path is resolved beneath.
fn open(root: &Path) -> Result<Dir, Error> {
    Dir::open_ambient_dir(root, ambient_authority())
        .map_err(|_| Error::Failed("the library's folder is unavailable".into()))
}

/// The folder holding `path`, a path [`relative`] accepted, and its last name. The folder is `.`
/// for a name at the root.
fn split(path: &Path) -> (&Path, &OsStr) {
    let folder = path.parent().filter(|p| !p.as_os_str().is_empty());
    (folder.unwrap_or(Path::new(".")), path.file_name().unwrap_or_default())
}

/// What `folder` holds (the root when it is empty), by name: the first [`PAGE`] entries whose
/// names sort after `after`, or from the first. Names sort by their UTF-8 bytes, and the folder
/// is read afresh each call, so an entry added or removed between calls moves no other. One
/// whose name is not UTF-8, which a plugin could not name, or that is gone before it is
/// described, is left out. Only a page of names is held at once, however large the folder.
pub fn entries(root: &Path, folder: &str, after: Option<&str>) -> Result<Vec<Entry>, Error> {
    let dir = if folder.is_empty() { Path::new(".") } else { relative(folder)? };
    let dir = open(root)?.open_dir(dir).map_err(|e| describe(folder, e))?;
    // The PAGE smallest names so far; the largest is at the top, to be dropped for a smaller.
    let mut page = BinaryHeap::with_capacity(PAGE + 1);
    for entry in dir.entries().map_err(|e| describe(folder, e))? {
        let entry = entry.map_err(|e| describe(folder, e))?;
        let Ok(name) = entry.file_name().into_string() else { continue };
        if after.is_none_or(|after| name.as_str() > after) {
            page.push(name);
            if page.len() > PAGE {
                page.pop();
            }
        }
    }
    let mut described = Vec::with_capacity(page.len());
    for name in page.into_sorted_vec() {
        // Metadata of the link itself: a plugin sees a symlink, and cannot follow it out.
        let meta = match dir.symlink_metadata(&name) {
            Ok(meta) => meta,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                let path = if folder.is_empty() { name } else { format!("{folder}/{name}") };
                return Err(describe(&path, e));
            }
        };
        let modified =
            meta.modified().ok().and_then(|t| t.into_std().duration_since(UNIX_EPOCH).ok());
        described.push(Entry {
            name,
            directory: meta.is_dir(),
            size: meta.len(),
            modified_ms: modified.map_or(0, |d| d.as_millis() as u64),
        });
    }
    Ok(described)
}

/// Up to `length` bytes of `path` from `offset`, and never more than [`MAX_READ`]; fewer at the
/// end of the file.
pub fn read(root: &Path, path: &str, offset: u64, length: u32) -> Result<Vec<u8>, Error> {
    let mut f = open(root)?.open(relative(path)?).map_err(|e| describe(path, e))?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| describe(path, e))?;
    let mut out = Vec::new();
    f.take(u64::from(length.min(MAX_READ))).read_to_end(&mut out).map_err(|e| describe(path, e))?;
    Ok(out)
}

/// Moves `from` to `to`, which must not exist, making the folders `to` is in (see [`move_folder`]
/// for the one case where an empty folder at `to` is replaced).
pub fn rename(root: &Path, from: &str, to: &str) -> Result<(), Error> {
    let (source, target) = (relative(from)?, relative(to)?);
    if target.starts_with(source) {
        return Err(Error::Refused(format!("{to} is {from}, or within it")));
    }
    let ((from_folder, from_name), (to_folder, to_name)) = (split(source), split(target));
    let root_dir = open(root)?;
    let from_dir = root_dir.open_dir(from_folder).map_err(|e| describe(from, e))?;
    // Looked at first so a missing source makes no folders.
    let meta = from_dir.symlink_metadata(from_name).map_err(|e| describe(from, e))?;
    root_dir.create_dir_all(to_folder).map_err(|e| describe(to, e))?;
    let to_dir = root_dir.open_dir(to_folder).map_err(|e| describe(to, e))?;
    let placed = if meta.is_dir() {
        move_folder(&from_dir, from_name, &to_dir, to_name).map(|()| Placed::Moved)
    } else {
        place_new(&from_dir, from_name, &to_dir, to_name)
    };
    let placed = placed.map_err(|e| match e.kind() {
        ErrorKind::AlreadyExists => describe(to, e),
        _ => describe(from, e),
    })?;
    moved(from, to, placed, || from_dir.remove_file(from_name))
}

/// Finishes moving `from` to `to` once it was `placed`: a file linked to its new name has its
/// old name removed with `remove`, and a removal that fails is reported.
fn moved(
    from: &str,
    to: &str,
    placed: Placed,
    remove: impl FnOnce() -> io::Result<()>,
) -> Result<(), Error> {
    match placed {
        Placed::Moved => Ok(()),
        Placed::Linked => remove().map_err(|e| {
            Error::Failed(format!("{to} was made, but {from} could not be removed: {}", e.kind()))
        }),
    }
}

/// Removes the file or empty folder at `path`, or a link there rather than what it leads to.
pub fn delete(root: &Path, path: &str) -> Result<(), Error> {
    let relative = relative(path)?;
    let root_dir = open(root)?;
    let meta = root_dir.symlink_metadata(relative).map_err(|e| describe(path, e))?;
    let removed =
        if meta.is_dir() { root_dir.remove_dir(relative) } else { root_dir.remove_file(relative) };
    removed.map_err(|e| describe(path, e))
}

/// An I/O failure, as a plugin sees it: a path `cap_std` found leading outside the root is
/// `refused` (it reports that as `PermissionDenied` with no OS error behind it); any other is
/// `failed`, as [`reason`] words it.
fn describe(path: &str, error: io::Error) -> Error {
    if error.kind() == ErrorKind::PermissionDenied && error.raw_os_error().is_none() {
        return Error::Refused(format!("{path} leads outside the library"));
    }
    Error::Failed(reason(path, &error))
}

/// An I/O failure in words a plugin author can act on, naming the path as the plugin gave it
/// and never the host's.
fn reason(path: &str, error: &io::Error) -> String {
    use std::io::ErrorKind::*;
    match error.kind() {
        NotFound => format!("{path} does not exist"),
        AlreadyExists => format!("{path} already exists"),
        PermissionDenied => format!("{path}: permission denied"),
        ReadOnlyFilesystem => format!("{path} cannot be written: the library is read-only here"),
        DirectoryNotEmpty => format!("{path} is not empty"),
        IsADirectory => format!("{path} is a folder"),
        NotADirectory => format!("{path} is not a folder"),
        _ => format!("{path}: {}", error.kind()),
    }
}

#[cfg(test)]
mod tests;
