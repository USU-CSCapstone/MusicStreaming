//! The library's folders, as plugins reach them (`requirements/plugins.md` §3): every path is
//! relative to one of the library's roots and is refused if it would leave it, by `..`, by
//! being absolute, or through a symlink.
//!
//! Writes appear whole or not at all: a file is written beside its target under a temporary
//! name, then linked or renamed into place.
//!
//! A path the host will not use is `refused`; one the filesystem turns down is `failed`.

use std::collections::BinaryHeap;
use std::fs;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::host::{Entry, Error};

/// The largest file a plugin writes in one call. It is held in the plugin's memory whole.
const MAX_WRITE: usize = 32 << 20;
/// The most a plugin reads in one call.
pub const MAX_READ: u32 = 8 << 20;
/// The most entries one call to [`entries`] answers, as many as a page of the library.
const PAGE: usize = 500;

/// `relative` within `root`, refused unless it is a plain relative path of one name or more.
fn resolve(root: &Path, relative: &str) -> Result<PathBuf, Error> {
    let path = Path::new(relative);
    if relative.contains('\0') || relative.contains('\\') {
        return Err(Error::Refused(format!("{relative:?} is not a library path")));
    }
    if path.components().next().is_none() {
        return Err(Error::Refused("the path is empty".into()));
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(Error::Refused(format!("{relative:?} is not a path within the library")));
        }
    }
    Ok(root.join(path))
}

/// Refuses `path` if, once symlinks are followed, it is not within `root`. Checks the nearest
/// part of it that exists, so a file about to be created is checked by its folder.
fn confined(root: &Path, path: &Path) -> Result<(), Error> {
    let unavailable = |_| Error::Failed("the library's folder is unavailable".into());
    let root = root.canonicalize().map_err(unavailable)?;
    let existing = path.ancestors().find(|p| p.exists()).unwrap_or(&root);
    let real = existing.canonicalize().map_err(|e| Error::Failed(e.to_string()))?;
    if real.starts_with(&root) {
        Ok(())
    } else {
        Err(Error::Refused("that path leads outside the library".into()))
    }
}

/// What `folder` holds, by name: the first [`PAGE`] entries whose names sort after `after`, or
/// from the first. Names sort by their UTF-8 bytes, and the folder is read afresh each call, so
/// an entry added or removed between calls moves no other. One whose name is not UTF-8, which
/// a plugin could not name, or that is gone before it is described, is left out. Only a page
/// of names is held at once, however large the folder.
pub fn entries(root: &Path, folder: &str, after: Option<&str>) -> Result<Vec<Entry>, Error> {
    let dir = if folder.is_empty() { root.to_path_buf() } else { resolve(root, folder)? };
    confined(root, &dir)?;
    // The PAGE smallest names so far; the largest is at the top, to be dropped for a smaller.
    let mut page = BinaryHeap::with_capacity(PAGE + 1);
    for entry in fs::read_dir(&dir).map_err(|e| describe(folder, e))? {
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
        let meta = match fs::symlink_metadata(dir.join(&name)) {
            Ok(meta) => meta,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => {
                let path = if folder.is_empty() { name } else { format!("{folder}/{name}") };
                return Err(describe(&path, e));
            }
        };
        let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok());
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
    let file = resolve(root, path)?;
    confined(root, &file)?;
    let mut f = fs::File::open(&file).map_err(|e| describe(path, e))?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| describe(path, e))?;
    let mut out = Vec::new();
    f.take(u64::from(length.min(MAX_READ))).read_to_end(&mut out).map_err(|e| describe(path, e))?;
    Ok(out)
}

/// Writes `contents` to `path`: a new file if `replace` is false, which fails if one is
/// there, or over an existing file if it is true, which fails if none is.
pub fn write(root: &Path, path: &str, contents: &[u8], replace: bool) -> Result<PathBuf, Error> {
    if contents.len() > MAX_WRITE {
        let refused = format!("a file written at once is at most {} MB", MAX_WRITE >> 20);
        return Err(Error::Refused(refused));
    }
    let target = resolve(root, path)?;
    let folder = target.parent().ok_or_else(|| Error::Refused("a file needs a name".into()))?;
    confined(root, &target)?;
    if replace && !target.is_file() {
        return Err(Error::Failed(format!("{path} does not exist")));
    }
    fs::create_dir_all(folder).map_err(|e| describe(path, e))?;
    // Checked again now the folders exist, in case one of them was a link.
    confined(root, &target)?;
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let staged = folder.join(format!(".{name}.{}.part", std::process::id()));
    let result = stage(&staged, contents).and_then(|()| {
        if replace {
            fs::rename(&staged, &target)
        } else {
            // A hard link fails if the target exists, so a file that appeared meanwhile is
            // never replaced.
            fs::hard_link(&staged, &target)
        }
    });
    fs::remove_file(&staged).ok();
    result.map_err(|e| describe(path, e))?;
    Ok(target)
}

fn stage(staged: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut f = fs::File::create_new(staged)?;
    f.write_all(contents)?;
    f.sync_all()
}

/// Moves `from` to `to`, which must not exist. Returns both, as the scanner needs to see each.
pub fn rename(root: &Path, from: &str, to: &str) -> Result<(PathBuf, PathBuf), Error> {
    let (source, target) = (resolve(root, from)?, resolve(root, to)?);
    confined(root, &source)?;
    confined(root, &target)?;
    if !source.exists() {
        return Err(Error::Failed(format!("{from} does not exist")));
    }
    if target.exists() {
        return Err(Error::Failed(format!("{to} already exists")));
    }
    let folder = target.parent().ok_or_else(|| Error::Refused("a file needs a name".into()))?;
    fs::create_dir_all(folder).map_err(|e| describe(to, e))?;
    confined(root, &target)?;
    fs::rename(&source, &target).map_err(|e| describe(from, e))?;
    Ok((source, target))
}

/// Removes the file or empty folder at `path`, or a link there rather than what it leads to.
/// Returns its path, for the scanner to see it gone.
pub fn delete(root: &Path, path: &str) -> Result<PathBuf, Error> {
    let target = resolve(root, path)?;
    confined(root, &target)?;
    let meta = fs::symlink_metadata(&target).map_err(|e| describe(path, e))?;
    let removed = if meta.is_dir() { fs::remove_dir(&target) } else { fs::remove_file(&target) };
    removed.map_err(|e| describe(path, e))?;
    Ok(target)
}

/// An I/O failure, as a `failed` in words a plugin author can act on, naming the path as the
/// plugin gave it and never the host's.
fn describe(path: &str, error: std::io::Error) -> Error {
    use std::io::ErrorKind::*;
    Error::Failed(match error.kind() {
        NotFound => format!("{path} does not exist"),
        AlreadyExists => format!("{path} already exists"),
        PermissionDenied => format!("{path}: permission denied"),
        ReadOnlyFilesystem => format!("{path} cannot be written: the library is read-only here"),
        DirectoryNotEmpty => format!("{path} is not empty"),
        IsADirectory => format!("{path} is a folder"),
        NotADirectory => format!("{path} is not a folder"),
        _ => format!("{path}: {}", error.kind()),
    })
}

#[cfg(test)]
mod tests;
