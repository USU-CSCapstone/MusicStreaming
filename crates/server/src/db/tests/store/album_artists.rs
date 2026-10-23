//! Album artists compared as a set, shown in the order most of the album's tracks list them
//! (`requirements/albums.md` §1–§2).

use jewelcase_scanner::testing::{self, ItemKey};
use jewelcase_scanner::{Batch, Store};

use super::recompute::check;
use super::{count, make, record, scalar, tagged};
use crate::db::store::SqliteStore;

/// The tags of track `number` of album "Duo" by album artists `artists`.
fn tags(artists: &'static str, number: &'static str) -> [(ItemKey, &'static str); 3] {
    [(ItemKey::AlbumTitle, "Duo"), (ItemKey::AlbumArtist, artists), (ItemKey::TrackNumber, number)]
}

/// The album artists' names, in order, joined with `;`.
fn listed(store: &SqliteStore) -> Option<String> {
    scalar(
        store,
        "SELECT group_concat(a.name, ';' ORDER BY aa.position) FROM album_artists aa \
         JOIN artists a ON a.id = aa.artist_id",
    )
}

/// Tracks listing the same album artists in any order, and in any case, are one album, which
/// lists them as most of its tracks do.
#[test]
fn album_artists_listed_in_another_order_are_the_same_album() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(&root, "D/1.wav", &tags("A;B", "1"));
    tagged(&root, "D/2.wav", &tags("B;A", "2"));
    tagged(&root, "D/3.wav", &tags("b;a", "3"));
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    check(&store, "the scan");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM albums"), 1);
    assert_eq!(listed(&store).unwrap(), "B;A");
}

/// A record naming the same album artist twice, in two spellings, lists it once, on the album
/// its single listing makes. The tag reader already drops such repeats; the store holds to it
/// for any record.
#[test]
fn an_album_artist_named_twice_is_listed_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    let listings: [&[&str]; 2] = [&["A", "a"], &["A"]];
    let upserts = listings.into_iter().enumerate().map(|(i, artists)| {
        let mut rec = record(&root, i);
        rec.tags.album_artists = artists.iter().map(|&a| a.to_owned()).collect();
        rec
    });
    store.apply(1, Batch { upserts: upserts.collect(), ..Batch::default() }).unwrap();
    check(&store, "the upserts");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM albums"), 1);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM track_album_artists"), 2);
    assert_eq!(
        count(&store, "SELECT COUNT(DISTINCT album_artist_ids) FROM tracks"),
        1,
        "both tracks list their album artists alike"
    );
    assert_eq!(listed(&store).unwrap(), "A");
}

/// Files with no album-artist tag take their primary artist, so a duet credited in either order
/// is two albums.
#[test]
fn untagged_duets_credited_in_either_order_split() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    for (rel, artists) in [("D/1.wav", "C;D"), ("D/2.wav", "D;C")] {
        tagged(&root, rel, &[(ItemKey::AlbumTitle, "Duet"), (ItemKey::TrackArtist, artists)]);
    }
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    check(&store, "the scan");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM albums"), 2);
}
