//! Tracks sorted by album or by artist follow the album's and the artist's sort key, not each
//! file's, so an album or an artist whose files disagree stays together (`design/database.md` §3).

use std::path::Path;

use jewelcase_scanner::*;

use super::recompute::check;
use super::{make, record};
use crate::db::store::SqliteStore;

/// Record `i` under `root` on album `album` by `artist`, its file giving them the sort keys
/// `album_sort` and `artist_sort`.
fn track(
    root: &Path,
    i: usize,
    (album, album_sort): (&str, &str),
    (artist, artist_sort): (&str, &str),
) -> TrackRecord {
    let mut rec = record(root, i);
    rec.tags.album = Some(album.to_owned());
    rec.tags.artists = vec![artist.to_owned()];
    rec.tags.album_artists = Vec::new();
    rec.sort.album = album_sort.to_owned();
    rec.sort.artist = artist_sort.to_owned();
    rec.sort.album_artist = artist_sort.to_owned();
    rec
}

/// The title of each track's album, and the name of its first artist, in the order `order` sorts
/// the tracks.
fn sorted(store: &SqliteStore, order: &'static str) -> Vec<(String, String)> {
    let sql = format!(
        "SELECT al.title, ar.name FROM tracks t JOIN albums al ON al.id = t.album_id \
         JOIN track_artists ta ON ta.track_id = t.id AND ta.position = 0 \
         JOIN artists ar ON ar.id = ta.artist_id ORDER BY {order}"
    );
    store
        .db()
        .read_blocking(move |conn| {
            conn.prepare(&sql)?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
        })
        .unwrap()
}

/// Album A's two files give it the sort keys "c" and "a", and album B's "b", which sorts between
/// them; A sorts as "a", the first among its tracks', though it was made with the first file's
/// "c", so sorted by album its tracks stay together. Artist X's files give it "x1" and "x3", and
/// artist Y's "x2"; X sorts as "x1", the first of its tied keys, so sorted by artist its tracks
/// stay together too. A file added later to each, giving them "d" and "x4", sorts with them.
#[test]
fn an_album_or_artist_whose_files_disagree_on_its_sort_key_sorts_together() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    let upserts = vec![
        track(&root, 0, ("A", "c"), ("X", "x3")),
        track(&root, 1, ("A", "a"), ("X", "x1")),
        track(&root, 2, ("B", "b"), ("Y", "x2")),
    ];
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    check(&store, "the upserts");

    let by = |album: &str, artist: &str| (album.to_owned(), artist.to_owned());
    let by_album = "t.album_sort_key, t.album_id, t.disc_number, t.id";
    assert_eq!(sorted(&store, by_album), [by("A", "X"), by("A", "X"), by("B", "Y")]);
    let by_artist = "t.artist_sort_key, t.sort_key, t.id";
    assert_eq!(sorted(&store, by_artist), [by("A", "X"), by("A", "X"), by("B", "Y")]);

    let upserts = vec![track(&root, 3, ("A", "d"), ("X", "x4"))];
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    check(&store, "the later file");
    let together = [by("A", "X"), by("A", "X"), by("A", "X"), by("B", "Y")];
    assert_eq!(sorted(&store, by_album), together);
    assert_eq!(sorted(&store, by_artist), together);
}
