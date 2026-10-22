//! Placing a file without replacing one, by the fallbacks a filesystem without
//! `RENAME_NOREPLACE` takes, and reporting a staged copy left behind.

use std::io;

use super::*;
use crate::files::place::link_new;
use crate::files::write::settle;

/// A staged copy holding `contents` in the library's `Album`, by the name `staged`.
fn staged(root: &Path, staged: &str, contents: &[u8]) {
    fs::write(root.join("Album").join(staged), contents).unwrap();
}

/// Without `RENAME_NOREPLACE`, a new file is linked into place and its staged copy removed; a
/// file already there is left as it was, and the staged copy is removed all the same.
#[test]
fn a_create_by_link_makes_the_file_once_and_leaves_no_staged_copy() {
    let (_temp, root) = library();
    let album = open(&root).unwrap().open_dir("Album").unwrap();
    let place = |from: &str| {
        let linked = link_new(&album, from.as_ref(), &album, "01.lrc".as_ref());
        settle("Album/01.lrc", from, linked.map(|()| Placed::Linked), || album.remove_file(from))
    };
    staged(&root, ".a.part", b"first");
    place(".a.part").unwrap();
    staged(&root, ".b.part", b"second");
    let clobber = place(".b.part");
    assert!(matches!(clobber, Err(Error::Failed(why)) if why == "Album/01.lrc already exists"));
    assert_eq!(fs::read(root.join("Album/01.lrc")).unwrap(), b"first");
    assert_eq!(names(&root, "Album", None), ["01.flac", "01.lrc"], "no staged copies left");
}

/// A staged copy that cannot be removed is reported, saying whether the file was written.
#[test]
fn a_staged_copy_left_behind_is_reported() {
    let stuck = || Err(io::Error::from(io::ErrorKind::PermissionDenied));
    let linked = settle("a.lrc", ".1.part", Ok(Placed::Linked), stuck);
    let written = "a.lrc was written, but its staged copy .1.part could not be removed";
    assert!(failed(linked, written));
    let unplaced = settle("a.lrc", ".1.part", Err(io::ErrorKind::AlreadyExists.into()), stuck);
    let both = "a.lrc already exists, and its staged copy .1.part could not be removed";
    assert!(failed(unplaced, both));
    let moved = settle("a.lrc", ".1.part", Ok(Placed::Moved), || panic!("nothing to remove"));
    assert!(moved.is_ok());
}

/// A folder moved into itself through a link fails as the filesystem words it, not as a
/// filesystem that cannot rename without replacing.
#[test]
fn a_folder_moved_into_itself_through_a_link_fails_plainly() {
    let (_temp, root) = library();
    symlink("Album", root.join("L")).unwrap();
    let moved = rename(&root, "Album", "L/Inside");
    let plain = "Album: invalid input parameter";
    assert!(matches!(&moved, Err(Error::Failed(why)) if why == plain), "{moved:?}");
    assert!(root.join("Album/01.flac").is_file());
}

/// A file moved by linking has its old name removed, and a removal that fails is reported.
#[test]
fn a_linked_move_removes_its_old_name() {
    let mut removed = false;
    moved("a.lrc", "b.lrc", Placed::Linked, || {
        removed = true;
        Ok(())
    })
    .unwrap();
    assert!(removed);
    let stuck = || Err(io::ErrorKind::PermissionDenied.into());
    let left = moved("a.lrc", "b.lrc", Placed::Linked, stuck);
    assert!(failed(left, "b.lrc was made, but a.lrc could not be removed: permission denied"));
    moved("a.lrc", "b.lrc", Placed::Moved, || panic!("nothing to remove")).unwrap();
}
