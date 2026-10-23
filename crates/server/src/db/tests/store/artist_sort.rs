//! An album sorted by artist follows its artist's sort key (`design/database.md` §3).

use std::path::Path;

use jewelcase_scanner::testing::InspectStore;
use jewelcase_scanner::*;

use super::recompute::check;
use super::{make, record};
use crate::db::store::SqliteStore;

/// Record `i` under `root`, as [`record`] makes it, with the artist sort tag `sort`.
fn track(root: &Path, i: usize, sort: &str) -> TrackRecord {
    let mut rec = record(root, i);
    rec.sort.artist = sort.into();
    rec
}

/// The artist sort key of the album titled `title`.
fn album_key(store: &SqliteStore, title: &'static str) -> Vec<u8> {
    let sql = "SELECT artist_sort_key FROM albums WHERE title = ?1";
    store.db().read_blocking(move |conn| conn.query_row(sql, [title], |r| r.get(0))).unwrap()
}

/// Retagging the artist's sort on one album re-sorts their other album too, which no change in
/// the batch touched.
#[test]
fn an_untouched_album_follows_its_artists_sort_key() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    let first = [0, 1, 10].map(|i| track(&root, i, "artist 0")).to_vec();
    store.apply(1, Batch { upserts: first, ..Batch::default() }).unwrap();
    assert_eq!(album_key(&store, "Album 1"), b"artist 0");

    let tracks = store.inspect_tracks(1);
    let retag = |i| {
        let rec = track(&root, i, "zero");
        let id = tracks.iter().find(|t| t.path == rec.path).map(|t| t.id);
        TrackRecord { id, ..rec }
    };
    let upserts = [0, 1].map(retag).to_vec();
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    check(&store, "the retag");
    assert_eq!(album_key(&store, "Album 0"), b"zero");
    assert_eq!(album_key(&store, "Album 1"), b"zero");
}
