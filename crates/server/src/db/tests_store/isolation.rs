//! A batch, result or placeholder applied for one library leaves another library's rows alone.

use jewelcase_scanner::testing::{self, InspectStore};
use jewelcase_scanner::*;

use super::{count, libraries, make, record, scalar};
use crate::db::testing::Image;

/// A batch returning a track, an analysis result and a placeholder, each applied for library 2,
/// leave library 1's track, waveform and image as they were and feed nothing to library 2.
#[test]
fn another_librarys_track_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    let other = tmp.path().join("other");
    store
        .db()
        .write_blocking(move |tx| {
            libraries::create(tx, Some(2), "Other", &[&other], &[])?;
            tx.execute_batch(
                "UPDATE tracks SET missing_since = 5;
                 INSERT INTO track_waveforms (library_id, track_id, data)
                 SELECT library_id, id, x'00' FROM tracks;",
            )
        })
        .unwrap();
    store.refresh_roots().unwrap();
    let track: TrackId = scalar(&store, "SELECT id FROM tracks").unwrap();

    let other: LibraryId = 2;
    let batch = Batch { returned: vec![track], changed_unread: vec![track], ..Batch::default() };
    store.apply(other, batch).unwrap();
    // No RMS levels: the path that deletes a waveform. The file is as picked, so only the library
    // stops the write.
    let result = AnalysisResult { integrated_lufs: Some(-9.0), ..AnalysisResult::empty(1) };
    let stamp = |sql| scalar::<u64>(&store, sql).unwrap();
    let (size, mtime_ms) =
        (stamp("SELECT file_size FROM tracks"), stamp("SELECT file_mtime FROM tracks"));
    store.store_analysis(other, PickedTrack { id: track, size, mtime_ms }, result).unwrap();
    // A placeholder made in library 2 for content only library 1 has an image of.
    store.db().write_blocking(|tx| Image::new(1, 1, "A/X/cover.png").insert(tx)).unwrap();
    let hash = 1_i64.to_be_bytes();
    store.store_placeholder(other, &hash, vec![1, 2, 3]).unwrap();

    let row = "SELECT missing_since, analyzer_version FROM tracks";
    let unchanged = store
        .db()
        .read_blocking(move |conn| {
            conn.query_row(row, [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        })
        .unwrap();
    assert_eq!(unchanged, (5, 0));
    assert_eq!(count(&store, "SELECT COUNT(*) FROM track_waveforms"), 1);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM images WHERE id = 1 AND placeholder IS NULL"),
        1
    );
    assert_eq!(store.feed_head(other), 0);
}

/// A batch applied for another library marks none of this one's tracks missing.
#[test]
fn another_librarys_batch_marks_nothing_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    let other = tmp.path().join("other");
    store
        .db()
        .write_blocking(move |tx| libraries::create(tx, Some(2), "Other", &[&other], &[]))
        .unwrap();
    store.refresh_roots().unwrap();
    let (track, head): (TrackId, i64) =
        (scalar(&store, "SELECT id FROM tracks").unwrap(), store.feed_head(lib.id));

    let batch = Batch { missing: vec![track], ..Batch::default() };
    assert_eq!(store.apply(2, batch).unwrap(), 0);
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));
    assert_eq!((store.feed_head(lib.id), store.feed_head(2)), (head, 0));
}

#[test]
fn an_upsert_naming_another_librarys_track_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    // A track with an artist, an album artist, and a genre.
    store.apply(lib.id, Batch { upserts: vec![record(&root, 0)], ..Batch::default() }).unwrap();
    let other_root = tmp.path().join("other");
    let created = other_root.clone();
    store
        .db()
        .write_blocking(move |tx| libraries::create(tx, Some(2), "Other", &[&created], &[]))
        .unwrap();
    store.refresh_roots().unwrap();
    let (before, head) = (store.inspect_tracks(lib.id), store.feed_head(lib.id));
    assert!(before[0].id > i64::from(u32::MAX), "a new track's id is drawn");

    // A record for a file under library 2's root that carries library 1's track ID.
    let other: LibraryId = 2;
    let mut batch = Batch::default();
    batch.upserts.push(TrackRecord { id: Some(before[0].id), ..record(&other_root, 0) });
    store.apply(other, batch).unwrap();

    assert_eq!(store.inspect_tracks(lib.id), before);
    assert_eq!(store.feed_head(lib.id), head);
    assert!(store.inspect_tracks(other).is_empty());
    let made = "SELECT (SELECT COUNT(*) FROM albums WHERE library_id = 2) \
                + (SELECT COUNT(*) FROM artists WHERE library_id = 2)";
    assert_eq!(count(&store, made), 0, "nothing made for it is left behind");
    let named = "SELECT COUNT(*) FROM library_changes c WHERE c.library_id = 2 AND ( \
                 (c.entity_type = 'artist' AND c.entity_id IN (SELECT id FROM artists WHERE library_id = 1)) \
                 OR (c.entity_type = 'tag' AND c.entity_id IN (SELECT id FROM tags WHERE library_id = 1)) \
                 OR (c.entity_type = 'album' AND c.entity_id IN (SELECT id FROM albums WHERE library_id = 1)))";
    assert_eq!(count(&store, named), 0, "library 2's feed names nothing of library 1's");
}

/// Two libraries holding an artist, album and genre of the same names keep a row of each per
/// library, each counting only its own library's tracks.
#[test]
fn the_same_names_in_two_libraries_are_rows_of_each() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, other) = (tmp.path().join("lib"), tmp.path().join("other"));
    std::fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let created = other.clone();
    store
        .db()
        .write_blocking(move |tx| libraries::create(tx, Some(2), "Other", &[&created], &[]))
        .unwrap();
    store.refresh_roots().unwrap();
    // Each library's first track is by "Artist 0" on "Album 0" in "Genre 0"; library 2 has a
    // second track on them too.
    store.apply(1, Batch { upserts: vec![record(&root, 0)], ..Batch::default() }).unwrap();
    let upserts = vec![record(&other, 0), record(&other, 3)];
    store.apply(2, Batch { upserts, ..Batch::default() }).unwrap();

    let counts = "SELECT group_concat(n, ' ' ORDER BY n) FROM (\
         SELECT library_id || ':' || track_count AS n FROM artists \
         UNION ALL SELECT library_id || ':' || track_count FROM albums \
         UNION ALL SELECT library_id || ':' || track_count FROM tags)";
    assert_eq!(scalar::<String>(&store, counts).unwrap(), "1:1 1:1 1:1 2:2 2:2 2:2");
}
