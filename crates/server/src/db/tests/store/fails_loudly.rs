//! A row that cannot be read or written fails loudly rather than reading as empty.

use jewelcase_scanner::testing;
use jewelcase_scanner::*;

use super::{libraries, make};

#[test]
fn unreadable_settings_fail_loudly_rather_than_read_as_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    // A valid JSON array, as the schema checks, but not of what it should hold.
    store
        .db()
        .write_blocking(|tx| tx.execute("UPDATE libraries SET excludes = '[1]'", []))
        .unwrap();
    assert!(store.db().read_blocking(libraries::all).is_err());
}

#[test]
fn a_scan_that_cannot_be_recorded_is_not_recorded_empty() {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    // A folder name that is not UTF-8, as Linux allows.
    let folder = root.join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
    let scopes = [Scope::folder(&root, folder, Depth::Directory)];
    assert!(store.create_scan(lib.id, Trigger::Watch, &scopes).is_err());
    assert!(store.scans(1).unwrap().is_empty());
}
