//! An absent album artist is the track's primary artist (`requirements/albums.md` §1).

use std::fs;

use jewelcase_scanner::testing::{self, ItemKey};

use super::recompute::check;
use super::{count, make, scalar, tagged};

/// A guest on one track of an untagged album neither splits it nor owns it: the album is its
/// primary artist's, with that artist's biography, and the guest only appears on it.
#[test]
fn a_guest_on_an_untagged_album_owns_none_of_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(&root, "A/X/1.wav", &[(ItemKey::TrackArtist, "A"), (ItemKey::AlbumTitle, "X")]);
    tagged(&root, "A/X/2.wav", &[(ItemKey::TrackArtist, "A;B"), (ItemKey::AlbumTitle, "X")]);
    fs::write(root.join("A/artist.txt"), "A's story").unwrap();
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    check(&store, "the scan");

    assert_eq!(count(&store, "SELECT COUNT(*) FROM albums"), 1);
    assert_eq!(scalar::<String>(&store, "SELECT artists_key FROM albums"), Some("a".into()));
    // Albums owned, albums appeared on, and biography.
    let artist = |name: &'static str| -> (i64, i64, Option<String>) {
        store
            .db()
            .read_blocking(move |conn| {
                conn.query_row(
                    "SELECT album_count, appearance_count, biography FROM artists WHERE name = ?1",
                    [name],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
            })
            .unwrap()
    };
    assert_eq!(artist("A"), (1, 0, Some("A's story".into())));
    assert_eq!(artist("B"), (0, 1, None));
}

/// Tracks by different artists of an album whose release type says compilation, with no album
/// artist and no compilation flag, make one album, of the unknown artist.
#[test]
fn a_compilation_by_release_type_is_one_album() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    for (rel, artist) in [("V/1.wav", "G"), ("V/2.wav", "H")] {
        tagged(
            &root,
            rel,
            &[
                (ItemKey::TrackArtist, artist),
                (ItemKey::AlbumTitle, "Hits"),
                (ItemKey::MusicBrainzReleaseType, "compilation"),
            ],
        );
    }
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    check(&store, "the scan");

    assert_eq!(count(&store, "SELECT COUNT(*) FROM albums"), 1);
    assert_eq!(scalar::<String>(&store, "SELECT artists_key FROM albums"), Some(String::new()));
    assert_eq!(scalar::<String>(&store, "SELECT type FROM albums"), Some("compilation".into()));
}
