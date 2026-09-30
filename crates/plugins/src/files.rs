//! The library's folders, as plugins reach them (`requirements/plugins.md` §3): every path is
//! relative to one of the library's roots and is refused if it would leave it, by `..`, by
//! being absolute, or through a symlink.
//!
//! Writes appear whole or not at all: a file is written beside its target under a temporary
//! name, then linked or renamed into place.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

/// The largest file a plugin writes in one call. It is held in the plugin's memory whole.
pub const MAX_WRITE: usize = 32 << 20;
/// The most a plugin reads in one call.
pub const MAX_READ: u32 = 8 << 20;
/// The most entries one listing answers.
const MAX_ENTRIES: usize = 10_000;

pub struct Entry {
    pub name: String,
    pub directory: bool,
    pub size: u64,
    pub modified_ms: u64,
}

/// `relative` within `root`, refused unless it is a plain relative path of one name or more.
fn resolve(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if relative.contains('\0') || relative.contains('\\') {
        return Err(format!("{relative:?} is not a library path"));
    }
    if path.components().next().is_none() {
        return Err("the path is empty".into());
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(format!("{relative:?} is not a path within the library"));
        }
    }
    Ok(root.join(path))
}

/// Refuses `path` if, once symlinks are followed, it is not within `root`. Checks the nearest
/// part of it that exists, so a file about to be created is checked by its folder.
fn confined(root: &Path, path: &Path) -> Result<(), String> {
    let root = root.canonicalize().map_err(|_| "the library's folder is unavailable".to_owned())?;
    let existing = path.ancestors().find(|p| p.exists()).unwrap_or(&root);
    let real = existing.canonicalize().map_err(|e| e.to_string())?;
    if real.starts_with(&root) { Ok(()) } else { Err("that path leads outside the library".into()) }
}

pub fn list(root: &Path, folder: &str) -> Result<Vec<Entry>, String> {
    let dir = if folder.is_empty() { root.to_path_buf() } else { resolve(root, folder)? };
    confined(root, &dir)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| describe(folder, e))? {
        let entry = entry.map_err(|e| describe(folder, e))?;
        // Metadata of the link itself: a plugin sees a symlink, and cannot follow it out.
        let Ok(meta) = entry.metadata() else { continue };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok());
        entries.push(Entry {
            name,
            directory: meta.is_dir(),
            size: meta.len(),
            modified_ms: modified.map_or(0, |d| d.as_millis() as u64),
        });
        if entries.len() == MAX_ENTRIES {
            break;
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

pub fn read(root: &Path, path: &str, offset: u64, length: u32) -> Result<Vec<u8>, String> {
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
pub fn write(root: &Path, path: &str, contents: &[u8], replace: bool) -> Result<PathBuf, String> {
    if contents.len() > MAX_WRITE {
        return Err(format!("a file written at once is at most {} MB", MAX_WRITE >> 20));
    }
    let target = resolve(root, path)?;
    let folder = target.parent().ok_or("a file needs a name")?;
    confined(root, &target)?;
    if replace && !target.is_file() {
        return Err(format!("{path} does not exist"));
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
pub fn rename(root: &Path, from: &str, to: &str) -> Result<(PathBuf, PathBuf), String> {
    let (source, target) = (resolve(root, from)?, resolve(root, to)?);
    confined(root, &source)?;
    confined(root, &target)?;
    if !source.exists() {
        return Err(format!("{from} does not exist"));
    }
    if target.exists() {
        return Err(format!("{to} already exists"));
    }
    let folder = target.parent().ok_or("a file needs a name")?;
    fs::create_dir_all(folder).map_err(|e| describe(to, e))?;
    confined(root, &target)?;
    fs::rename(&source, &target).map_err(|e| describe(from, e))?;
    Ok((source, target))
}

pub fn delete(root: &Path, path: &str) -> Result<PathBuf, String> {
    let target = resolve(root, path)?;
    confined(root, &target)?;
    let meta = fs::symlink_metadata(&target).map_err(|e| describe(path, e))?;
    let removed = if meta.is_dir() { fs::remove_dir(&target) } else { fs::remove_file(&target) };
    removed.map_err(|e| describe(path, e))?;
    Ok(target)
}

/// An I/O failure in words a plugin author can act on, naming the path as the plugin gave it
/// and never the host's.
fn describe(path: &str, error: std::io::Error) -> String {
    use std::io::ErrorKind::*;
    match error.kind() {
        NotFound => format!("{path} does not exist"),
        AlreadyExists => format!("{path} already exists"),
        PermissionDenied | ReadOnlyFilesystem => {
            format!("{path} cannot be written: the library is read-only here")
        }
        DirectoryNotEmpty => format!("{path} is not empty"),
        IsADirectory => format!("{path} is a folder"),
        NotADirectory => format!("{path} is not a folder"),
        _ => format!("{path}: {}", error.kind()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("music");
        fs::create_dir_all(root.join("Album")).unwrap();
        fs::write(root.join("Album/01.flac"), b"audio").unwrap();
        (temp, root)
    }

    #[test]
    fn paths_stay_within_the_root() {
        let (temp, root) = library();
        for bad in ["/etc/passwd", "../outside", "Album/../../outside", "Album\\x", "a\0b", ""] {
            assert!(write(&root, bad, b"x", false).is_err(), "{bad:?}");
        }
        fs::write(temp.path().join("secret"), b"host file").unwrap();
        std::os::unix::fs::symlink(temp.path(), root.join("escape")).unwrap();
        assert!(write(&root, "escape/planted", b"x", false).is_err());
        assert!(read(&root, "escape/secret", 0, 10).is_err());
        // A link that leads back inside the library is simply a path within it.
        assert_eq!(read(&root, "escape/music/Album/01.flac", 0, 5).unwrap(), b"audio");
        assert!(list(&root, "escape").is_err());
        assert!(!temp.path().join("planted").exists());
    }

    #[test]
    fn creating_never_replaces_and_replacing_needs_a_file() {
        let (_temp, root) = library();
        let made = write(&root, "Album/01.lrc", b"[00:01]x", false).unwrap();
        assert_eq!(made, root.join("Album/01.lrc"));
        assert!(
            write(&root, "Album/01.lrc", b"other", false).unwrap_err().contains("already exists")
        );
        assert_eq!(fs::read(&made).unwrap(), b"[00:01]x");
        write(&root, "Album/01.lrc", b"new", true).unwrap();
        assert_eq!(fs::read(&made).unwrap(), b"new");
        assert!(write(&root, "Album/02.lrc", b"x", true).unwrap_err().contains("does not exist"));
        let names: Vec<_> = list(&root, "Album").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["01.flac", "01.lrc"], "no staged files left behind");
    }

    #[test]
    fn writes_make_their_folders() {
        let (_temp, root) = library();
        write(&root, "New Artist/New Album/01.flac", b"audio", false).unwrap();
        let listed = list(&root, "").unwrap();
        assert!(listed.iter().any(|e| e.name == "New Artist" && e.directory));
    }

    #[test]
    fn reads_a_range() {
        let (_temp, root) = library();
        assert_eq!(read(&root, "Album/01.flac", 1, 3).unwrap(), b"udi");
        assert_eq!(read(&root, "Album/01.flac", 3, 100).unwrap(), b"io");
        assert!(read(&root, "Album/nothing", 0, 1).unwrap_err().contains("does not exist"));
    }

    #[test]
    fn renames_and_deletes_within_the_root() {
        let (_temp, root) = library();
        write(&root, "Album/cover.jpg", b"jpeg", false).unwrap();
        assert!(
            rename(&root, "Album/01.flac", "Album/cover.jpg")
                .unwrap_err()
                .contains("already exists")
        );
        rename(&root, "Album/01.flac", "Other/01.flac").unwrap();
        assert!(root.join("Other/01.flac").is_file());
        assert!(delete(&root, "Album").unwrap_err().contains("not empty"));
        delete(&root, "Album/cover.jpg").unwrap();
        delete(&root, "Album").unwrap();
        assert!(delete(&root, "").is_err());
        assert!(!root.join("Album").exists());
    }
}
