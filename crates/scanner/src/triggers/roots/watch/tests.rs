use std::cell::Cell;
use std::fs::Permissions;
use std::os::unix::fs::{PermissionsExt, symlink};

use super::*;
use crate::discover::compile_excludes;
use crate::scope::Depth;

/// A root holding the folders `a`, `a/x`, `b` and `skip/y`, a file, and a link to `a`.
fn tree() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for folder in ["a/x", "b", "skip/y"] {
        fs::create_dir_all(root.join(folder)).unwrap();
    }
    fs::write(root.join("file"), b"x").unwrap();
    symlink(root.join("a"), root.join("link")).unwrap();
    tmp
}

/// What a walk did: the folders it tried, sorted, and what it returned.
type Walked = (Vec<PathBuf>, notify::Result<()>);

/// Walks `scope`, excluding `skip`, with `add` answering for each folder.
fn walk(scope: &Scope, add: impl Fn(&Path) -> notify::Result<()>) -> Walked {
    let excludes = compile_excludes(&["skip".into()]).unwrap();
    let mut tried = Vec::new();
    let (_alive, stop) = Stop::new();
    let result = watch_folders(scope, &excludes, &stop, |folder| {
        tried.push(folder.to_path_buf());
        add(folder)
    });
    tried.sort();
    (tried, result)
}

/// The paths of `folders` in `root`.
fn paths(root: &Path, folders: &[&str]) -> Vec<PathBuf> {
    folders.iter().map(|folder| root.join(folder)).collect()
}

/// An error watching `folder` for want of permission.
fn refused(folder: &Path) -> notify::Error {
    notify::Error::io(io::ErrorKind::PermissionDenied.into()).add_path(folder.into())
}

#[test]
fn every_folder_the_scan_walks_is_watched_and_nothing_else() {
    let tmp = tree();
    let root = tmp.path();
    let (tried, result) = walk(&Scope::root(root), |_| Ok(()));
    assert!(result.is_ok());
    assert_eq!(tried, [vec![root.to_path_buf()], paths(root, &["a", "a/x", "b"])].concat());
    let in_excluded = Scope::folder(root, root.join("skip/y"), Depth::Subtree);
    assert_eq!(walk(&in_excluded, |_| Ok(())).0, Vec::<PathBuf>::new());
}

/// A folder gone by the time it is watched or listed is passed over.
#[test]
fn a_folder_gone_is_passed_over() {
    let tmp = tree();
    let root = tmp.path();
    let a = root.join("a");
    let gone_at_a = |folder: &Path| {
        let error = notify::Error::new(ErrorKind::PathNotFound).add_path(folder.into());
        if folder == a { Err(error) } else { Ok(()) }
    };
    let (tried, result) = walk(&Scope::root(root), gone_at_a);
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(tried, [vec![root.to_path_buf()], paths(root, &["a", "b"])].concat());
    let gone = Scope::folder(root, root.join("gone"), Depth::Subtree);
    assert!(walk(&gone, |_| Ok(())).1.is_ok(), "a folder gone before it was listed");
}

/// A folder that cannot be read is passed over, as the scan passes over it, unless it is the
/// root. Skipped, saying so, where permissions do not stop this user reading a folder, as for
/// root.
#[test]
fn an_unreadable_folder_is_passed_over() {
    let tmp = tree();
    let root = tmp.path();
    let a = root.join("a");
    fs::set_permissions(&a, Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&a).is_ok();
    let refused_at_a = |folder: &Path| if folder == a { Err(refused(folder)) } else { Ok(()) };
    let walked = (!readable).then(|| {
        (walk(&Scope::root(root), refused_at_a), walk(&Scope::root(&a), |f| Err(refused(f))))
    });
    fs::set_permissions(&a, Permissions::from_mode(0o755)).unwrap();
    let Some(((tried, result), (_, as_root))) = walked else {
        eprintln!("skipped: permissions do not stop this user reading a folder (running as root?)");
        return;
    };
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(tried, [vec![root.to_path_buf()], paths(root, &["a", "b"])].concat());
    assert!(as_root.is_err(), "an unreadable root was passed over");
}

/// A folder the scan can read but that may not be watched, as a security policy may refuse,
/// leaves the root not wholly watched, while every other folder is watched; so does a root
/// that may not be watched.
#[test]
fn a_readable_folder_that_may_not_be_watched_is_a_failure() {
    let tmp = tree();
    let root = tmp.path();
    let a = root.join("a");
    let (tried, result) =
        walk(&Scope::root(root), |folder| if folder == a { Err(refused(folder)) } else { Ok(()) });
    assert!(result.is_err(), "a readable folder was passed over");
    assert_eq!(tried, [vec![root.to_path_buf()], paths(root, &["a", "b"])].concat());

    let (tried, result) = walk(&Scope::root(root), |folder| Err(refused(folder)));
    assert_eq!(tried, [root]);
    assert!(result.is_err(), "the root was passed over");
}

/// Any other failure is returned once the walk has tried every other folder.
#[test]
fn a_folder_that_fails_otherwise_leaves_the_rest_watched() {
    let tmp = tree();
    let root = tmp.path();
    let x = root.join("a/x");
    let failing = |folder: &Path| {
        if folder == x { Err(notify::Error::generic("failed")) } else { Ok(()) }
    };
    let (tried, result) = walk(&Scope::root(root), failing);
    assert!(matches!(result.unwrap_err().kind, ErrorKind::Generic(_)));
    assert_eq!(tried, [vec![root.to_path_buf()], paths(root, &["a", "a/x", "b"])].concat());
}

/// The OS limit on watches, met at the second folder, ends the walk there.
#[test]
fn the_limit_on_watches_ends_the_walk() {
    let tmp = tree();
    let tries = Cell::new(0);
    let limited = |_: &Path| {
        tries.set(tries.get() + 1);
        if tries.get() == 2 { Err(notify::Error::new(ErrorKind::MaxFilesWatch)) } else { Ok(()) }
    };
    let (tried, result) = walk(&Scope::root(tmp.path()), limited);
    assert!(limit(&result.unwrap_err()));
    assert_eq!(tried.len(), 2, "{tried:?}");
}

/// A walk whose stop is asked as it watches the first folder ends there, leaving the other
/// folders of a long tree unwatched.
#[test]
fn a_stop_ends_the_walk_between_folders() {
    let tmp = tempfile::tempdir().unwrap();
    for n in 0..100 {
        fs::create_dir_all(tmp.path().join(format!("{n}/inner"))).unwrap();
    }
    let (alive, stop) = Stop::new();
    let mut alive = Some(alive);
    let mut tried = 0;
    let result = watch_folders(&Scope::root(tmp.path()), &GlobSet::empty(), &stop, |_| {
        tried += 1;
        drop(alive.take());
        Ok(())
    });
    assert!(result.is_ok());
    assert_eq!(tried, 1, "the walk went on after the stop");
}
