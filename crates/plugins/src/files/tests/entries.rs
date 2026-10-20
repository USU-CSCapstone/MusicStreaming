//! Listing a folder a page at a time: by name, after a name, and unmoved by changes between
//! pages.

use super::*;

/// The most pages [`every_name`] reads before it gives up, so paging that never ends fails.
const MOST_PAGES: usize = 10;

/// Every name `folder` holds after `after`, read a page at a time, and how many pages it took.
/// Panics past [`MOST_PAGES`].
fn every_name(root: &Path, folder: &str, after: Option<&str>) -> (Vec<String>, usize) {
    let mut all: Vec<String> = Vec::new();
    for pages in 0..MOST_PAGES {
        let page = names(root, folder, all.last().map(String::as_str).or(after));
        if page.is_empty() {
            return (all, pages);
        }
        all.extend(page);
    }
    panic!("{folder} was still not listed whole after {MOST_PAGES} pages");
}

/// A folder `name` in `root` holding empty files named `0000` to the number before `count`.
fn numbered(root: &Path, name: &str, count: usize) -> Vec<String> {
    let folder = root.join(name);
    fs::create_dir(&folder).unwrap();
    let names: Vec<_> = (0..count).map(|n| format!("{n:04}")).collect();
    for name in &names {
        fs::write(folder.join(name), b"").unwrap();
    }
    names
}

#[test]
fn a_listed_link_is_described_not_followed() {
    let (temp, root) = library();
    std::os::unix::fs::symlink(temp.path(), root.join("escape")).unwrap();
    let listed = entries(&root, "", None).unwrap();
    let link = listed.iter().find(|e| e.name == "escape").expect("the link is listed");
    assert!(!link.directory, "a link to a folder is not listed as one");
    assert_eq!(names(&root, "", None), ["Album", "escape"]);
}

#[test]
fn a_folder_is_listed_by_name_a_page_at_a_time() {
    let (_temp, root) = library();
    let all = numbered(&root, "Flat", 2 * PAGE + 34);
    assert_eq!(names(&root, "Flat", None), all[..PAGE]);
    assert_eq!(names(&root, "Flat", Some(&all[PAGE - 1])), all[PAGE..2 * PAGE]);
    assert_eq!(names(&root, "Flat", Some(&all[2 * PAGE - 1])), all[2 * PAGE..]);
    assert_eq!(every_name(&root, "Flat", None), (all, 3));
}

#[test]
fn past_the_end_is_empty() {
    let (_temp, root) = library();
    let all = numbered(&root, "Flat", 3);
    assert_eq!(names(&root, "Flat", Some(&all[2])), Vec::<String>::new());
    assert_eq!(names(&root, "Flat", Some("9999")), Vec::<String>::new());
    assert_eq!(names(&root, "Flat", Some("\u{10FFFF}")), Vec::<String>::new());
}

#[test]
fn names_sort_by_their_bytes_and_after_need_not_name_one() {
    let (_temp, root) = library();
    for name in ["é", "z", "Z", "b.flac", "a"] {
        fs::write(root.join("Album").join(name), b"").unwrap();
    }
    let all = ["01.flac", "Z", "a", "b.flac", "z", "é"];
    assert_eq!(names(&root, "Album", None), all);
    assert_eq!(names(&root, "Album", Some("b")), ["b.flac", "z", "é"]);
    assert_eq!(names(&root, "Album", Some("z")), ["é"]);
}

#[test]
fn a_folder_changing_between_pages_moves_no_other_entry() {
    let (_temp, root) = library();
    let all = numbered(&root, "Flat", 2 * PAGE + 100);
    let first = names(&root, "Flat", None);
    let last = first.last().unwrap().clone();
    // One gone before it was listed, one added among those listed, and one among those not yet.
    fs::remove_file(root.join("Flat").join(&all[PAGE + 10])).unwrap();
    fs::write(root.join("Flat/0001a"), b"").unwrap();
    fs::write(root.join("Flat/0700a"), b"").unwrap();
    let (rest, _) = every_name(&root, "Flat", Some(&last));
    let mut expected: Vec<_> =
        all[PAGE..].iter().filter(|n| **n != all[PAGE + 10]).cloned().collect();
    expected.push("0700a".into());
    expected.sort();
    assert_eq!(rest, expected, "each entry there throughout comes once, and in order");
}

#[test]
fn a_folder_of_exactly_a_page_is_listed_in_one() {
    let (_temp, root) = library();
    let all = numbered(&root, "Flat", PAGE);
    assert_eq!(names(&root, "Flat", None), all);
    assert_eq!(names(&root, "Flat", Some(&all[PAGE - 1])), Vec::<String>::new());
}

#[test]
fn an_entry_that_cannot_be_described_fails_the_listing_naming_it() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, root) = library();
    let album = root.join("Album");
    // Readable but not searchable: its names list, but none of them can be looked at.
    fs::set_permissions(&album, fs::Permissions::from_mode(0o444)).unwrap();
    if fs::symlink_metadata(album.join("01.flac")).is_ok() {
        fs::set_permissions(&album, fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("skipping: permissions do not apply to this user");
        return;
    }
    let listed = entries(&root, "Album", None);
    fs::set_permissions(&album, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(failed(listed, "Album/01.flac: permission denied"));
}

#[test]
fn a_name_that_is_not_utf8_is_left_out() {
    use std::os::unix::ffi::OsStrExt;
    let (_temp, root) = library();
    fs::write(root.join("Album").join(std::ffi::OsStr::from_bytes(b"bad\xff")), b"").unwrap();
    // The name a lossy reading would give it belongs to another entry, listed once.
    fs::write(root.join("Album/bad\u{FFFD}"), b"").unwrap();
    assert_eq!(names(&root, "Album", None), ["01.flac", "bad\u{FFFD}"]);
}
