//! The folder read against a real database: what it holds, and that it seeks the file index in
//! path order and sorts nothing.

use std::path::PathBuf;

use jewelcase_scanner::IndexedFolder;
use rusqlite::{Connection, Row, params};

use super::{SqliteStore, indexed_in, select};
use crate::db::testing::{self, Album, Track, library};

/// A store whose library 1 holds a track at each of `paths` under its root, the last one missing,
/// with that root, and the directory everything lives in.
fn store(paths: &[&'static str]) -> (tempfile::TempDir, PathBuf, SqliteStore) {
    let (dir, db) = testing::open();
    let root = dir.path().join("music");
    let (made, paths) = (root.clone(), paths.to_vec());
    db.write_blocking(move |tx| {
        library(tx, 1, "Music", made)?;
        Album::new(1, 1).insert(tx)?;
        for (id, path) in (1..).zip(&paths) {
            let missing_since = (paths.last() == Some(path)).then_some(1);
            Track { path: path.to_string(), missing_since, ..Track::new(id, 1, 1) }.insert(tx)?;
        }
        Ok(())
    })
    .unwrap();
    (dir, root, SqliteStore::new(db).unwrap())
}

/// A folder holds the files directly in it, missing or not, and names each folder below it once,
/// in path order: one that sorts before a file of the same stem, a sibling whose name extends
/// it, and one with a name longer in bytes than in characters. A sibling of the folder whose name
/// extends the folder's own is not in it, and a folder no root of the library holds holds nothing.
#[test]
fn a_folder_holds_its_own_files_and_names_its_folders() {
    let (dir, root, store) = store(&[
        "A/1.flac",
        "A/X 2/4.flac",
        "A/X.flac",
        "A/X/2.flac",
        "A/X/Y/3.flac",
        "A/Z.flac",
        "A/Ä/5.flac",
        "A0/6.flac",
        "B/7.flac",
    ]);
    let read = |rel: &str| indexed_in(&store, 1, &root.join(rel)).unwrap();

    let a = read("A");
    let mut files: Vec<&str> = a.files.keys().map(String::as_str).collect();
    files.sort_unstable();
    assert_eq!(files, ["1.flac", "X.flac", "Z.flac"]);
    assert_eq!(a.folders, ["X 2", "X", "Ä"]);
    assert_eq!((a.files["X.flac"].track_id, a.files["X.flac"].missing), (3, false));
    let top = indexed_in(&store, 1, &root).unwrap();
    assert_eq!((top.files.len(), top.folders), (0, vec!["A".to_owned(), "A0".into(), "B".into()]));
    assert_eq!(read("A/X").folders, ["Y"]);
    assert!(read("B").files["7.flac"].missing);
    assert_eq!(read("C"), IndexedFolder::default());
    assert_eq!(
        indexed_in(&store, 1, &dir.path().join("elsewhere")).unwrap(),
        IndexedFolder::default()
    );
    assert_eq!(indexed_in(&store, 2, &root.join("A")).unwrap(), IndexedFolder::default());
}

/// The steps of the plan for the folder read, with the range bounded above or not.
fn plan(conn: &Connection, bounded: bool) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {}", select(bounded)))?;
    let step = |row: &Row<'_>| row.get(3);
    if bounded {
        stmt.query_map(params![1, "A/", "A0"], step)?.collect()
    } else {
        stmt.query_map(params![1, ""], step)?.collect()
    }
}

/// Both forms of the read seek a range of the `(root_id, path)` index, in its order: nothing is
/// scanned and nothing sorted.
#[test]
fn the_read_seeks_the_file_index_and_sorts_nothing() {
    let (_dir, _root, store) = store(&["A/1.flac"]);
    for bounded in [true, false] {
        let steps = store.db().read_blocking(move |conn| plan(conn, bounded)).unwrap();
        let seeks = "SEARCH tracks USING INDEX tracks_file (root_id=? AND path>?";
        assert!(steps.iter().any(|s| s.starts_with(seeks)), "{steps:?}");
        assert!(
            steps.iter().all(|s| !s.starts_with("SCAN") && !s.contains("TEMP B-TREE")),
            "{steps:?}"
        );
    }
}
