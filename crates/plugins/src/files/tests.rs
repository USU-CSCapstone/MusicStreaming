use super::*;

mod entries;

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
    for bad in ["/etc/passwd", "../outside", "Album/../../outside", "Album\\x", "a\0b", ""] {
        assert!(refused(write(&root, bad, b"x", false)), "{bad:?}");
    }
    fs::write(temp.path().join("secret"), b"host file").unwrap();
    std::os::unix::fs::symlink(temp.path(), root.join("escape")).unwrap();
    // Refused before its folders are made, so nothing appears outside.
    assert!(refused(write(&root, "escape/new/planted", b"x", false)));
    assert!(refused(read(&root, "escape/secret", 0, 10)));
    // A link that leads back inside the library is simply a path within it.
    assert_eq!(read(&root, "escape/music/Album/01.flac", 0, 5).unwrap(), b"audio");
    assert!(refused(entries(&root, "escape", None)));
    assert!(refused(entries(&root, "escape", Some("music"))));
    assert!(refused(delete(&root, "escape/secret")));
    assert!(refused(rename(&root, "escape/secret", "Album/stolen")));
    assert!(refused(rename(&root, "Album/01.flac", "escape/new/moved")));
    std::os::unix::fs::symlink(temp.path().join("secret"), root.join("linked")).unwrap();
    assert!(refused(write(&root, "linked", b"x", true)));
    assert!(root.join("linked").is_symlink(), "the link is not replaced");
    assert_eq!(fs::read(temp.path().join("secret")).unwrap(), b"host file");
    assert!(root.join("Album/01.flac").is_file(), "a refused rename leaves its source");
    assert!(!root.join("Album/stolen").exists(), "a refused rename makes nothing");
    assert!(!temp.path().join("new").exists(), "nothing is made outside the library");
}

#[test]
fn creating_never_replaces_and_replacing_needs_a_file() {
    let (_temp, root) = library();
    let made = write(&root, "Album/01.lrc", b"[00:01]x", false).unwrap();
    assert_eq!(made, root.join("Album/01.lrc"));
    assert!(failed(write(&root, "Album/01.lrc", b"other", false), "already exists"));
    assert_eq!(fs::read(&made).unwrap(), b"[00:01]x");
    write(&root, "Album/01.lrc", b"new", true).unwrap();
    assert_eq!(fs::read(&made).unwrap(), b"new");
    assert!(failed(write(&root, "Album/02.lrc", b"x", true), "does not exist"));
    assert_eq!(names(&root, "Album", None), ["01.flac", "01.lrc"], "no staged files left behind");
}

#[test]
fn writes_make_their_folders() {
    let (_temp, root) = library();
    write(&root, "New Artist/New Album/01.flac", b"audio", false).unwrap();
    let listed = entries(&root, "", None).unwrap();
    assert!(listed.iter().any(|e| e.name == "New Artist" && e.directory));
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
    write(&root, "Album/cover.jpg", b"jpeg", false).unwrap();
    assert!(failed(rename(&root, "Album/01.flac", "Album/cover.jpg"), "already exists"));
    assert!(failed(rename(&root, "Album/nothing", "Album/other"), "does not exist"));
    rename(&root, "Album/01.flac", "Other/01.flac").unwrap();
    assert!(root.join("Other/01.flac").is_file());
    assert!(failed(delete(&root, "Album"), "not empty"));
    delete(&root, "Album/cover.jpg").unwrap();
    delete(&root, "Album").unwrap();
    assert!(refused(delete(&root, "")));
    assert!(!root.join("Album").exists());
}

#[test]
fn a_file_over_the_write_limit_is_refused() {
    let (_temp, root) = library();
    assert!(refused(write(&root, "Album/large", &vec![0; MAX_WRITE + 1], false)));
    assert!(!root.join("Album/large").exists());
}

#[test]
fn a_root_that_cannot_be_reached_has_failed() {
    let (temp, _root) = library();
    let gone = temp.path().join("unmounted");
    assert!(failed(entries(&gone, "", None), "unavailable"));
    assert!(failed(read(&gone, "Album/01.flac", 0, 1), "unavailable"));
}
