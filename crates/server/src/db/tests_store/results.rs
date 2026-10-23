//! What a scan stores beside the tags: placeholders, lyrics, artwork, problems, and analysis
//! results.

use std::sync::Arc;

use jewelcase_scanner::testing::{self, ItemKey};
use jewelcase_scanner::*;

use super::recompute::check;
use super::{count, make, scalar, tagged};
use crate::db::store::SqliteStore;
use crate::db::waveform;

#[test]
fn a_placeholder_is_an_album_change() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    std::fs::write(root.join("A/X/cover.png"), testing::PNG).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    let album_seq = |store: &SqliteStore| {
        scalar::<i64>(store, "SELECT seq FROM library_changes WHERE entity_type = 'album'").unwrap()
    };
    let (hash, _) = store.next_without_placeholder(lib.id, 1).unwrap().remove(0);

    // One that could not be made changes nothing clients see.
    let before = album_seq(&store);
    store.store_placeholder(lib.id, &hash, Vec::new()).unwrap();
    assert_eq!(album_seq(&store), before);
    store.store_placeholder(lib.id, &hash, vec![1, 2, 3]).unwrap();
    assert!(album_seq(&store) > before);
}

#[test]
fn lyrics_and_artwork_land_in_their_tables() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(
        &root,
        "a/01.wav",
        &[(ItemKey::TrackTitle, "1"), (ItemKey::TrackArtist, "A"), (ItemKey::AlbumTitle, "X")],
    );
    std::fs::write(root.join("a/01.lrc"), "[00:01.00]first\n[00:02.50]second").unwrap();
    // Not square, so a width and height stored the wrong way round show.
    std::fs::write(root.join("a/cover.png"), testing::PNG).unwrap();
    std::fs::copy(root.join("a/cover.png"), root.join("a/artist.png")).unwrap();
    std::fs::write(root.join("a/artist.txt"), "A biography.").unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(scalar::<String>(&store, "SELECT lyrics_kind FROM tracks"), Some("synced".into()));
    let synced: String = scalar(&store, "SELECT synced FROM track_lyrics").unwrap();
    let lines: Vec<serde_json::Value> = serde_json::from_str(&synced).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["startMs"], 2500);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM images"),
        1,
        "cover and artist image are the same bytes: one row"
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT width || 'x' || height FROM images"),
        Some("2x1".into())
    );
    assert!(scalar::<i64>(&store, "SELECT image_id FROM albums").is_some());
    assert!(scalar::<i64>(&store, "SELECT image_id FROM artists WHERE name = 'A'").is_some());
    assert_eq!(
        scalar::<String>(&store, "SELECT biography FROM artists WHERE name = 'A'"),
        Some("A biography.".into())
    );
}

#[test]
fn problems_group_by_kind_and_clear() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::write(root.join("a/one.flac"), b"garbage one").unwrap();
    std::fs::write(root.join("a/two.flac"), b"garbage two").unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM scan_problem_groups"), 1, "one systemic issue");
    assert_eq!(scalar::<i64>(&store, "SELECT count FROM scan_problem_groups"), Some(2));
    check(&store, "two problems");
    testing::write_small_wav(&root.join("a/one.flac"));
    testing::scan_library(&store, &lib);
    assert_eq!(scalar::<i64>(&store, "SELECT count FROM scan_problem_groups"), Some(1));
    check(&store, "one cleared");
    std::fs::remove_file(root.join("a/two.flac")).unwrap();
    testing::scan_library(&store, &lib);
    check(&store, "the other gone");
    // A deleted file takes its problem with it, and the emptied group goes.
    assert_eq!(count(&store, "SELECT COUNT(*) FROM scan_problem_groups"), 0);
}

#[test]
fn analysis_results_reach_tracks_albums_and_waveforms() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::write_sine_wav(&root.join("a/01.wav"), 1000.0, 0.5, 1.0);
    testing::tag_wav(&root.join("a/01.wav"), "1", "A", "X");
    let store = Arc::new(make(&root));
    let lib = testing::library(&root);
    testing::scan_library(store.as_ref(), &lib);
    let ffmpeg = jewelcase_ffmpeg::Ffmpeg::new(jewelcase_ffmpeg::Config::default()).unwrap();
    let analyzer = jewelcase_scanner::Analyzer::new(ffmpeg, store.clone());
    let report = analyzer.run_once(lib.id, 10);
    assert_eq!(report.done, 1);
    assert_eq!(analyzer.run_once(lib.id, 10).done, 0, "nothing left");
    let lufs: f64 = scalar(&store, "SELECT loudness_lufs FROM tracks").unwrap();
    assert!((lufs - -6.7).abs() < 1.0, "{lufs}");
    assert_eq!(
        scalar::<i64>(&store, "SELECT analyzer_version FROM tracks"),
        Some(jewelcase_scanner::ANALYZER_VERSION as i64)
    );
    let album_lufs: f64 = scalar(&store, "SELECT loudness_lufs FROM albums").unwrap();
    assert!((album_lufs - lufs).abs() < 0.01, "a one-track album is as loud as its track");
    let blob: Vec<u8> = scalar(&store, "SELECT data FROM track_waveforms").unwrap();
    assert_eq!(waveform::decode(&blob).unwrap().len(), 512);
}
