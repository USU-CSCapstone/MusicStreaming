//! How scanned tracks group into albums: across folders, without tags, and as compilations, EPs
//! and albums.

use jewelcase_scanner::testing::{self, ItemKey};
use rusqlite::Result;

use super::{count, make, scalar, tagged};

#[test]
fn album_split_across_folders_is_one_album() -> Result<()> {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(
        &root,
        "disc1/01.wav",
        &[
            (ItemKey::TrackTitle, "A"),
            (ItemKey::TrackArtist, "Band"),
            (ItemKey::AlbumTitle, "Live"),
            (ItemKey::DiscNumber, "1"),
            (ItemKey::TrackTotal, "10"),
        ],
    );
    tagged(
        &root,
        "disc2/01.wav",
        &[
            (ItemKey::TrackTitle, "B"),
            (ItemKey::TrackArtist, "Band"),
            (ItemKey::AlbumTitle, "live"),
            (ItemKey::DiscNumber, "2"),
            (ItemKey::TrackTotal, "8"),
        ],
    );
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM albums"),
        1,
        "case-folded title, same artists: one album"
    );
    assert_eq!(scalar::<i64>(&store, "SELECT track_count FROM albums"), Some(2));
    assert_eq!(scalar::<i64>(&store, "SELECT disc_count FROM albums"), Some(2));
    assert_eq!(
        scalar::<i64>(&store, "SELECT track_total FROM albums"),
        Some(18),
        "the sum of its discs' totals"
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM album_discs"), 2);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM album_artists"), 1);
    assert_eq!(
        scalar::<i64>(&store, "SELECT album_count FROM artists WHERE name_key = 'band'"),
        Some(1)
    );
    assert_eq!(scalar::<i64>(&store, "SELECT track_count FROM libraries WHERE id = 1"), Some(2));
    Ok(())
}

#[test]
fn untagged_files_share_the_unknown_artist_and_album() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::write_small_wav(&root.join("a/one.wav"));
    testing::write_small_wav(&root.join("b/two.wav"));
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM artists"), 1);
    assert_eq!(
        scalar::<Option<String>>(&store, "SELECT name FROM artists"),
        Some(None),
        "the unknown artist has no name"
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM albums"), 1);
    assert_eq!(scalar::<Option<String>>(&store, "SELECT title FROM albums"), Some(None));
    assert_eq!(scalar::<i64>(&store, "SELECT track_count FROM albums"), Some(2));
    assert_eq!(
        scalar::<String>(&store, "SELECT title FROM tracks WHERE path = 'a/one.wav'"),
        Some("one".into())
    );
}

#[test]
fn compilations_and_album_types() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(
        &root,
        "c/01.wav",
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "A"),
            (ItemKey::AlbumTitle, "Hits"),
            (ItemKey::AlbumArtist, "Various Artists"),
            (ItemKey::FlagCompilation, "1"),
        ],
    );
    tagged(
        &root,
        "c/02.wav",
        &[
            (ItemKey::TrackTitle, "2"),
            (ItemKey::TrackArtist, "B"),
            (ItemKey::AlbumTitle, "Hits"),
            (ItemKey::AlbumArtist, "Various Artists"),
            (ItemKey::FlagCompilation, "1"),
        ],
    );
    tagged(
        &root,
        "e/01.wav",
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "C"),
            (ItemKey::AlbumTitle, "Short"),
            (ItemKey::TrackTotal, "5"),
        ],
    );
    tagged(
        &root,
        "s/01.wav",
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "D"),
            (ItemKey::AlbumTitle, "One"),
            (ItemKey::TrackTotal, "2"),
            (ItemKey::MusicBrainzReleaseType, "album"),
        ],
    );
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM albums WHERE title = 'Hits'"),
        1,
        "one compilation, not one per artist"
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT type FROM albums WHERE title = 'Hits'"),
        Some("compilation".into())
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT type FROM albums WHERE title = 'Short'"),
        Some("ep".into())
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT type FROM albums WHERE title = 'One'"),
        Some("album".into()),
        "the tag beats the size"
    );
    // A and B appear on Hits but do not own it (`requirements/artists.md` §2).
    assert_eq!(
        scalar::<i64>(&store, "SELECT appearance_count FROM artists WHERE name = 'A'"),
        Some(1)
    );
    assert_eq!(scalar::<i64>(&store, "SELECT album_count FROM artists WHERE name = 'A'"), Some(0));
    assert_eq!(
        scalar::<i64>(&store, "SELECT album_count FROM artists WHERE name = 'Various Artists'"),
        Some(1)
    );
}
