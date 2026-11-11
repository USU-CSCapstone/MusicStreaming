//! What a scan stores beside the tags: placeholders, lyrics, artwork, problems, and analysis
//! results.

use std::sync::Arc;

use jewelcase_scanner::testing::{self, ItemKey};
use jewelcase_scanner::*;

use super::recompute::check;
use super::{count, make, scalar, tagged};
use crate::db::store::SqliteStore;
use crate::db::testing::Image;
use crate::db::waveform;

/// A placeholder is a change of the album showing its image, which brings it to every device,
/// and of a track only when the track's own image is not its album's.
#[test]
fn a_placeholder_is_an_album_change_and_a_track_change_only_for_its_own_art() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 2);
    std::fs::write(root.join("A/X/cover.png"), testing::PNG).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    // The second track shows art of its own.
    store
        .db()
        .write_blocking(move |tx| {
            Image::new(99, lib.id, "A/X/02.png").insert(tx)?;
            tx.execute(
                "UPDATE tracks SET image_id = 99 WHERE id = (SELECT max(id) FROM tracks)",
                [],
            )
            .map(drop)
        })
        .unwrap();
    let seq = |store: &SqliteStore, sql: &'static str| scalar::<i64>(store, sql).unwrap_or(0);
    let album = "SELECT max(seq) FROM library_changes WHERE entity_type = 'album'";
    let shared = "SELECT seq FROM library_changes WHERE entity_type = 'track' \
                  AND entity_id = (SELECT min(id) FROM tracks)";
    let own = "SELECT seq FROM library_changes WHERE entity_type = 'track' \
               AND entity_id = (SELECT max(id) FROM tracks)";
    let cover: Vec<u8> =
        scalar(&store, "SELECT hash FROM images WHERE id = (SELECT image_id FROM albums)").unwrap();

    // One that could not be made changes nothing clients see.
    let before = (seq(&store, album), seq(&store, shared), seq(&store, own));
    store.store_placeholder(lib.id, &cover, Vec::new()).unwrap();
    assert_eq!((seq(&store, album), seq(&store, shared), seq(&store, own)), before);
    store.store_placeholder(lib.id, &cover, vec![1, 2, 3]).unwrap();
    assert!(seq(&store, album) > before.0);
    assert_eq!(seq(&store, shared), before.1, "the album's change carries its tracks' image");

    store.store_placeholder(lib.id, &99_i64.to_be_bytes(), vec![4, 5, 6]).unwrap();
    assert!(seq(&store, own) > before.2, "a track's rows carry its own image's placeholder");
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
    let report = analyzer.run_once(lib.id, 10, &Stop::never());
    assert_eq!(report.done, 1);
    assert_eq!(analyzer.run_once(lib.id, 10, &Stop::never()).done, 0, "nothing left");
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
