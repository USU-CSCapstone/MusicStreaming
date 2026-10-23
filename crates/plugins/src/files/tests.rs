use std::fs;
use std::os::unix::fs::symlink;

use super::*;
use crate::host::WriteMode::{Create, Replace};

mod entries;
mod place;
mod races;

/// A library holding `Album/01.flac` in a temporary folder: that folder, and the library's root.
fn library() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("music");
    fs::create_dir_all(root.join("Album")).unwrap();
    fs::write(root.join("Album/01.flac"), b"audio").unwrap();
    (temp, root)
}

/// Whether `result` failed as `failed`, with a message containing `text`.
fn failed<T>(result: Result<T, Error>, text: &str) -> bool {
    matches!(result, Err(Error::Failed(why)) if why.contains(text))
}

/// Whether `result` was refused.
fn refused<T>(result: Result<T, Error>) -> bool {
    matches!(result, Err(Error::Refused(_)))
}

/// The names of the entries `folder` holds after `after`, one page.
fn names(root: &Path, folder: &str, after: Option<&str>) -> Vec<String> {
    entries(root, folder, after).unwrap().into_iter().map(|e| e.name).collect()
}

#[test]
fn paths_stay_within_the_root() {
    let (temp, root) = library();
    let outside = temp.path().join("secret");
    fs::write(&outside, b"host file").unwrap();
    for bad in ["/etc/passwd", "../outside", "Album/../../secret", "./Album/01.flac", "a\\x"] {
        assert!(refused(resolve(&root, bad)), "{bad:?}");
        assert!(refused(write(&root, bad, b"x", Create)), "{bad:?}");
        assert!(refused(read(&root, bad, 0, 10)), "{bad:?}");
        assert!(refused(entries(&root, bad, None)), "{bad:?}");
        assert!(refused(delete(&root, bad)), "{bad:?}");
        assert!(refused(rename(&root, bad, "Album/moved")), "{bad:?}");
        assert!(refused(rename(&root, "Album/01.flac", bad)), "{bad:?}");
    }
    assert!(refused(write(&root, "a\0b", b"x", Create)));
    assert!(refused(write(&root, "", b"x", Create)));
    symlink(temp.path(), root.join("escape")).unwrap();
    // Refused before its folders are made, so nothing appears outside.
    assert!(refused(write(&root, "escape/new/planted", b"x", Create)));
    assert!(refused(read(&root, "escape/secret", 0, 10)));
    // A link out of the library is refused even where the path leads back inside it.
    assert!(refused(read(&root, "escape/music/Album/01.flac", 0, 5)));
    assert!(refused(entries(&root, "escape", None)));
    assert!(refused(entries(&root, "escape", Some("music"))));
    assert!(refused(delete(&root, "escape/secret")));
    assert!(refused(rename(&root, "escape/secret", "Album/stolen")));
    assert!(refused(rename(&root, "Album/01.flac", "escape/new/moved")));
    symlink(&outside, root.join("linked")).unwrap();
    assert!(refused(write(&root, "linked", b"x", Replace)));
    assert!(root.join("linked").is_symlink(), "the link is not replaced");
    assert_eq!(fs::read(&outside).unwrap(), b"host file");
    assert!(root.join("Album/01.flac").is_file(), "a refused rename leaves its source");
    assert!(!root.join("Album/stolen").exists(), "a refused rename makes nothing");
    assert!(!temp.path().join("new").exists(), "nothing is made outside the library");
}

#[test]
fn a_link_is_followed_only_while_it_stays_within_the_root() {
    let (_temp, root) = library();
    symlink("Album", root.join("Inner")).unwrap();
    symlink("../../music/Album/01.flac", root.join("Album/out")).unwrap();
    assert_eq!(read(&root, "Inner/01.flac", 0, 5).unwrap(), b"audio");
    write(&root, "Inner/cover.jpg", b"jpeg", Create).unwrap();
    assert!(root.join("Album/cover.jpg").is_file());
    // Leaving the root even for a moment, or by an absolute link to within it, is refused.
    symlink(root.join("Album/01.flac"), root.join("absolute")).unwrap();
    assert!(refused(read(&root, "absolute", 0, 5)));
    assert!(refused(read(&root, "Inner/out", 0, 5)));
}

#[test]
fn creating_never_replaces_and_replacing_needs_a_file() {
    let (_temp, root) = library();
    write(&root, "Album/01.lrc", b"[00:01]x", Create).unwrap();
    let made = resolve(&root, "Album/01.lrc").unwrap();
    assert_eq!(made, root.join("Album/01.lrc"));
    assert!(failed(write(&root, "Album/01.lrc", b"other", Create), "already exists"));
    assert_eq!(fs::read(&made).unwrap(), b"[00:01]x");
    write(&root, "Album/01.lrc", b"new", Replace).unwrap();
    assert_eq!(fs::read(&made).unwrap(), b"new");
    assert!(failed(write(&root, "Album/02.lrc", b"x", Replace), "does not exist"));
    assert!(failed(write(&root, "Nowhere/02.lrc", b"x", Replace), "does not exist"));
    assert!(!root.join("Nowhere").exists(), "replacing makes no folders");
    assert!(failed(write(&root, "Album", b"x", Replace), "is not a file"));
    assert_eq!(names(&root, "Album", None), ["01.flac", "01.lrc"], "no staged files left behind");
}

#[test]
fn writes_make_their_folders() {
    let (_temp, root) = library();
    write(&root, "New Artist/New Album/01.flac", b"audio", Create).unwrap();
    write(&root, "notes.txt", b"at the root", Create).unwrap();
    // The longest name most filesystems allow, which the staged copy's name does not lengthen.
    let longest = format!("Album/{}", "n".repeat(250));
    write(&root, &longest, b"long", Create).unwrap();
    assert_eq!(fs::read(root.join(&longest)).unwrap(), b"long");
    let listed = entries(&root, "", None).unwrap();
    assert!(listed.iter().any(|e| e.name == "New Artist" && e.directory));
    assert_eq!(fs::read(root.join("notes.txt")).unwrap(), b"at the root");
}

#[test]
fn reads_a_range() {
    let (_temp, root) = library();
    assert_eq!(read(&root, "Album/01.flac", 1, 3).unwrap(), b"udi");
    assert_eq!(read(&root, "Album/01.flac", 3, 100).unwrap(), b"io");
    assert!(failed(read(&root, "Album/nothing", 0, 1), "does not exist"));
}

#[test]
fn renames_and_deletes_within_the_root() {
    let (_temp, root) = library();
    write(&root, "Album/cover.jpg", b"jpeg", Create).unwrap();
    let clobber = rename(&root, "Album/01.flac", "Album/cover.jpg");
    assert!(failed(clobber, "Album/cover.jpg already exists"));
    assert!(failed(rename(&root, "Album/nothing", "Other/01.flac"), "does not exist"));
    assert!(!root.join("Other").exists(), "a missing source makes no folders");
    assert!(refused(rename(&root, "Album", "Album/Inside")));
    rename(&root, "Album/01.flac", "Other/01.flac").unwrap();
    assert!(root.join("Other/01.flac").is_file());
    assert!(failed(delete(&root, "Album"), "not empty"));
    // A link to a folder is removed itself, and the folder is left as it was.
    symlink("Album", root.join("L")).unwrap();
    delete(&root, "L").unwrap();
    assert!(!root.join("L").is_symlink());
    assert!(root.join("Album/cover.jpg").is_file());
    delete(&root, "Album/cover.jpg").unwrap();
    delete(&root, "Album").unwrap();
    assert!(refused(delete(&root, "")));
    assert!(!root.join("Album").exists());
}

#[test]
fn a_file_over_the_write_limit_is_refused() {
    let (_temp, root) = library();
    assert!(refused(write(&root, "Album/large", &vec![0; MAX_WRITE + 1], Create)));
    assert!(!root.join("Album/large").exists());
}

#[test]
fn a_root_that_cannot_be_reached_has_failed() {
    let (temp, _root) = library();
    let gone = temp.path().join("unmounted");
    assert!(failed(entries(&gone, "", None), "unavailable"));
    assert!(failed(read(&gone, "Album/01.flac", 0, 1), "unavailable"));
}
