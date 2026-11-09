//! Which changes reach plugins through the change feed: each entity's feed row keeps the
//! position plugins last saw it change at (`0009_plugin_changes.sql`), which moves only when
//! what the plugin API reads of it changes, while the row's own position moves for devices.

use std::path::Path;

use jewelcase_scanner::testing::{self, InspectStore};
use jewelcase_scanner::*;

use super::{make, record};
use crate::db::catalog::{self, Touched};
use crate::db::store::SqliteStore;

/// The feed positions, for devices and for plugins, of the `kind` entity that `id` selects.
fn positions(store: &SqliteStore, kind: &'static str, id: &'static str) -> (i64, i64) {
    let sql = format!(
        "SELECT seq, plugin_seq FROM library_changes WHERE entity_type = ?1 AND entity_id = ({id})"
    );
    store
        .db()
        .read_blocking(move |conn| conn.query_row(&sql, [kind], |r| Ok((r.get(0)?, r.get(1)?))))
        .unwrap()
}

/// Asserts that the change between `before` and `after` reached devices, and plugins only if
/// `plugins` says so.
fn moved(before: (i64, i64), after: (i64, i64), plugins: bool, what: &str) {
    assert!(after.0 > before.0, "{what}: devices hear of it: {before:?} -> {after:?}");
    assert_eq!(after.1 > before.1, plugins, "{what}: plugins woken: {before:?} -> {after:?}");
}

/// Stores records `ids` of [`record`], each edited by `edit`, as new files or as retagged files
/// of tracks already stored.
fn apply(store: &SqliteStore, root: &Path, ids: &[usize], edit: impl Fn(&mut TrackRecord)) {
    let stored = store.inspect_tracks(1);
    let upserts = ids
        .iter()
        .map(|&i| {
            let mut rec = record(root, i);
            edit(&mut rec);
            rec.id = stored.iter().find(|t| t.path == rec.path).map(|t| t.id);
            rec
        })
        .collect();
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
}

/// A stored image placeholder changes what devices show of the album and track showing it, and
/// nothing a plugin reads.
#[test]
fn a_placeholder_wakes_no_plugin() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    std::fs::write(root.join("A/X/cover.png"), testing::PNG).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    let (hash, _) = store.next_without_placeholder(lib.id, 1).unwrap().remove(0);
    let track = || positions(&store, "track", "SELECT id FROM tracks");
    let album = || positions(&store, "album", "SELECT id FROM albums");
    let before = (track(), album());

    store.store_placeholder(lib.id, &hash, vec![1, 2, 3]).unwrap();
    moved(before.0, track(), false, "the track");
    moved(before.1, album(), false, "the album");
}

/// An artist's new sort key, copied to a track the batch did not touch, changes how devices sort
/// it and nothing a plugin reads, while a track retitled in the same batch wakes them.
#[test]
fn an_artist_sort_key_wakes_no_plugin_for_its_tracks_but_a_retitle_does() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    apply(&store, &root, &[0, 1, 10], |_| ());
    let untouched = "SELECT id FROM tracks WHERE title = 'Track 10'";
    let retitled = "SELECT id FROM tracks WHERE title LIKE 'Track 0%'";
    let before = (positions(&store, "track", untouched), positions(&store, "track", retitled));

    apply(&store, &root, &[0, 1], |rec| {
        rec.sort.artist = "zero".into();
        rec.sort.album_artist = "zero".into();
        if rec.tags.title.as_deref() == Some("Track 0") {
            rec.tags.title = Some("Track 0, retitled".into());
        }
    });
    moved(before.0, positions(&store, "track", untouched), false, "the untouched track");
    moved(before.1, positions(&store, "track", retitled), true, "the retitled track");
}

/// An album title that flips to another spelling of itself changes what plugins see of every
/// track on it, the ones the batch did not touch too, though no sort key moved.
#[test]
fn an_album_titles_new_spelling_reaches_tracks_outside_the_batch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    apply(&store, &root, &[0, 1, 2], |_| ());
    let untouched = "SELECT id FROM tracks WHERE title = 'Track 2'";
    let before = positions(&store, "track", untouched);

    apply(&store, &root, &[0, 1], |rec| rec.tags.album = Some("ALBUM 0".into()));
    let title = "SELECT title FROM albums";
    let title: String =
        store.db().read_blocking(move |c| c.query_row(title, [], |r| r.get(0))).unwrap();
    assert_eq!(title, "ALBUM 0", "the album is spelled as most of its tracks spell it");
    moved(before, positions(&store, "track", untouched), true, "the untouched track");
}

/// An artist renamed to another spelling of itself, its sort key unchanged, changes what plugins
/// see of every album it is an album artist of, the ones the batch did not touch too.
#[test]
fn an_artists_new_spelling_reaches_albums_outside_the_batch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    apply(&store, &root, &[0, 1, 10], |_| ());
    let untouched = "SELECT id FROM albums WHERE title = 'Album 1'";
    let before = positions(&store, "album", untouched);
    let key = |store: &SqliteStore| -> Vec<u8> {
        let sql = "SELECT sort_key FROM artists";
        store.db().read_blocking(move |c| c.query_row(sql, [], |r| r.get(0))).unwrap()
    };
    let old_key = key(&store);

    apply(&store, &root, &[0, 1], |rec| {
        rec.tags.artists = vec!["ARTIST 0".into()];
        rec.tags.album_artists = vec!["ARTIST 0".into()];
    });
    let name = "SELECT name FROM artists";
    let name: String =
        store.db().read_blocking(move |c| c.query_row(name, [], |r| r.get(0))).unwrap();
    assert_eq!(name, "ARTIST 0", "the artist is spelled as most of its credits spell it");
    assert_eq!(key(&store), old_key, "its sort key did not move");
    moved(before, positions(&store, "album", untouched), true, "the untouched album");
}

/// Adding a track to an album, its going missing and coming back, and its deletion each move the
/// album's counts, so each reaches plugins as an album change; retitling a track moves none of
/// them, so it reaches them as a track change alone.
#[test]
fn an_albums_tracks_coming_and_going_wake_plugins_for_the_album() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    apply(&store, &root, &[0, 1, 2], |_| ());
    let album = || positions(&store, "album", "SELECT id FROM albums");
    let third = || -> TrackId {
        let sql = "SELECT id FROM tracks WHERE title = 'Track 3'";
        store.db().read_blocking(move |c| c.query_row(sql, [], |r| r.get(0))).unwrap()
    };

    let before = album();
    apply(&store, &root, &[3], |_| ());
    moved(before, album(), true, "a track added");

    let before = album();
    store.apply(1, Batch { missing: vec![third()], ..Batch::default() }).unwrap();
    moved(before, album(), true, "a track missing");

    let before = album();
    store.apply(1, Batch { returned: vec![third()], ..Batch::default() }).unwrap();
    moved(before, album(), true, "a track back");

    let before = album();
    let id = third();
    store
        .db()
        .write_blocking(move |tx| {
            let mut touched = Touched::default();
            touched.track(tx, 1, id)?;
            tx.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
            catalog::recompute(tx, 1, &touched, 0)
        })
        .unwrap();
    moved(before, album(), true, "a track deleted");

    let before = album();
    apply(&store, &root, &[0], |rec| rec.tags.title = Some("Track 0, retitled".into()));
    let after = album();
    assert_eq!(after.1, before.1, "a retitled track wakes no album hook: {before:?} -> {after:?}");
}
