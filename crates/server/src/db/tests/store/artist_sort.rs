//! An album sorted by artist follows its artist's sort key (`design/database.md` §3).

use std::path::Path;

use jewelcase_scanner::testing::{self, InspectStore, ItemKey};
use jewelcase_scanner::*;

use super::recompute::check;
use super::{make, record, tagged};
use crate::db::store::SqliteStore;

/// Record `i` under `root`, as [`record`] makes it, with the artist sort tag `sort`.
fn track(root: &Path, i: usize, sort: &str) -> TrackRecord {
    let mut rec = record(root, i);
    rec.sort.artist = sort.into();
    rec.sort.album_artist = sort.into();
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

/// The sort key of the artist named `name`.
fn artist_key(store: &SqliteStore, name: &'static str) -> Vec<u8> {
    let sql = "SELECT sort_key FROM artists WHERE name = ?1";
    store.db().read_blocking(move |conn| conn.query_row(sql, [name], |r| r.get(0))).unwrap()
}

/// An artist credited only as an album artist, such as a composer, sorts by its album artist sort
/// tag, and so does its album.
#[test]
fn an_album_artist_only_artist_sorts_by_its_album_artist_sort_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let tags = [
        (ItemKey::TrackArtist, "Performer"),
        (ItemKey::AlbumArtist, "Johann Sebastian Bach"),
        (ItemKey::AlbumArtistSortOrder, "Bach, Johann Sebastian"),
        (ItemKey::AlbumTitle, "Suites"),
    ];
    tagged(&root, "Suites/1.wav", &tags);
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    check(&store, "the scan");
    assert_eq!(artist_key(&store, "Johann Sebastian Bach"), b"bach, johann sebastian");
    assert_eq!(album_key(&store, "Suites"), b"bach, johann sebastian");
    assert_eq!(artist_key(&store, "Performer"), b"performer");
}

/// With no album artist tagged, the album artist is the primary artist, whose votes then come
/// from `ARTISTSORT` alone, whatever `ALBUMARTISTSORT` says.
#[test]
fn without_album_artists_the_primary_artist_sorts_by_its_artist_sort_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let tags = [
        (ItemKey::TrackArtist, "Solo"),
        (ItemKey::TrackArtistSortOrder, "Alone"),
        (ItemKey::AlbumArtistSortOrder, "Aardvark"),
        (ItemKey::AlbumTitle, "One"),
    ];
    tagged(&root, "One/1.wav", &tags);
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    check(&store, "the scan");
    assert_eq!(artist_key(&store, "Solo"), b"alone");
    assert_eq!(album_key(&store, "One"), b"alone");
}

/// A file whose album artist is its artist votes for one key with both credits: `ARTISTSORT`, or
/// `ALBUMARTISTSORT` when that is the only tag, never the name against the tag. The tags sort
/// after the names so a tie by bytes would pick the name.
#[test]
fn an_artist_that_is_also_the_album_artist_sorts_by_whichever_tag_the_file_has() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let files =
        [("Dylan", ItemKey::TrackArtistSortOrder), ("Zappa", ItemKey::AlbumArtistSortOrder)];
    for (i, (name, key)) in files.into_iter().enumerate() {
        for n in 1..=2 {
            let tag = format!("Zz {name}");
            let tags = [
                (ItemKey::TrackArtist, name),
                (ItemKey::AlbumArtist, name),
                (ItemKey::AlbumTitle, if i == 0 { "First" } else { "Second" }),
                (key, tag.as_str()),
            ];
            tagged(&root, &format!("{name}/{n}.wav"), &tags);
        }
    }
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    check(&store, "the scan");
    assert_eq!(artist_key(&store, "Dylan"), b"zz dylan");
    assert_eq!(artist_key(&store, "Zappa"), b"zz zappa");
    assert_eq!(album_key(&store, "First"), b"zz dylan");
}

/// A compilation's unknown album artist takes no sort tag from its tracks' artist.
#[test]
fn the_unknown_album_artist_of_a_compilation_has_no_sort_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let tags = [
        (ItemKey::TrackArtist, "A"),
        (ItemKey::TrackArtistSortOrder, "Zz"),
        (ItemKey::FlagCompilation, "1"),
        (ItemKey::AlbumTitle, "Mix"),
    ];
    tagged(&root, "Mix/1.wav", &tags);
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    check(&store, "the scan");
    let sql = "SELECT sort_key FROM artists WHERE name IS NULL";
    let key: Vec<u8> =
        store.db().read_blocking(move |conn| conn.query_row(sql, [], |r| r.get(0))).unwrap();
    assert_eq!(key, b"");
}
