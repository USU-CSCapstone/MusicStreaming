//! An artist's name is the most-used spelling of it, and a retag moves a track and removes the
//! album and artist it emptied.

use jewelcase_scanner::testing;
use lofty::tag::ItemKey;

use super::{count, make, scalar, tagged};

#[test]
fn most_used_spelling_wins_and_ties_break_by_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(
        &root,
        "a/01.wav",
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "nirvana"),
            (ItemKey::AlbumTitle, "X"),
        ],
    );
    tagged(
        &root,
        "a/02.wav",
        &[
            (ItemKey::TrackTitle, "2"),
            (ItemKey::TrackArtist, "Nirvana"),
            (ItemKey::AlbumTitle, "X"),
        ],
    );
    tagged(
        &root,
        "a/03.wav",
        &[
            (ItemKey::TrackTitle, "3"),
            (ItemKey::TrackArtist, "Nirvana"),
            (ItemKey::AlbumTitle, "X"),
        ],
    );
    // A tie elsewhere: one each, "Beck" sorts before "beck" by bytes.
    tagged(
        &root,
        "b/01.wav",
        &[(ItemKey::TrackTitle, "1"), (ItemKey::TrackArtist, "beck"), (ItemKey::AlbumTitle, "Y")],
    );
    tagged(
        &root,
        "b/02.wav",
        &[(ItemKey::TrackTitle, "2"), (ItemKey::TrackArtist, "Beck"), (ItemKey::AlbumTitle, "Y")],
    );
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM artists"),
        2,
        "spellings fold to one artist each"
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT name FROM artists WHERE name_key = 'nirvana'"),
        Some("Nirvana".into())
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT name FROM artists WHERE name_key = 'beck'"),
        Some("Beck".into())
    );
    assert_eq!(
        scalar::<i64>(&store, "SELECT track_count FROM artists WHERE name_key = 'nirvana'"),
        Some(3)
    );
}

#[test]
fn retag_moves_track_and_removes_emptied_album_and_artist() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(
        &root,
        "x/01.wav",
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "Old"),
            (ItemKey::AlbumTitle, "First"),
            (ItemKey::Genre, "Rock"),
        ],
    );
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM artists WHERE name = 'Old'"), 1);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM tags"), 1);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let p = root.join("x/01.wav");
    testing::tag_wav_with(
        &p,
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "New"),
            (ItemKey::AlbumTitle, "Second"),
            (ItemKey::Genre, "Jazz"),
        ],
    );
    std::fs::File::open(&p)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    let scan = testing::scan_library(&store, &lib);
    assert_eq!(scan.progress.updated, 1);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM tracks"), 1);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM artists"), 1, "Old is gone");
    assert_eq!(scalar::<String>(&store, "SELECT name FROM artists"), Some("New".into()));
    assert_eq!(count(&store, "SELECT COUNT(*) FROM albums"), 1);
    assert_eq!(scalar::<String>(&store, "SELECT title FROM albums"), Some("Second".into()));
    assert_eq!(scalar::<String>(&store, "SELECT name FROM tags"), Some("Jazz".into()));
    // The feed carries tombstones for what vanished and one row per entity.
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM library_changes WHERE entity_type = 'artist' AND op = 'delete'"
        ),
        1
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM library_changes WHERE entity_type = 'track'"),
        1
    );
    assert_eq!(scalar::<i64>(&store, "SELECT artist_count FROM libraries WHERE id = 1"), Some(1));
}
