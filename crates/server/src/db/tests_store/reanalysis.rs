//! What a rescan leaves of a track's analysis: its loudness and peak, its waveform, and the
//! loudness of its album.

use std::fs;

use jewelcase_scanner::analysis::ANALYZER_VERSION;
use jewelcase_scanner::*;

use super::{make, scalar};
use crate::db::store::SqliteStore;

/// One track's stored analysis: its path, analyzer version, loudness, peak, and whether it has
/// a waveform.
type Stored = (String, i64, Option<f64>, Option<f64>, bool);

/// Every track's stored analysis, by path.
fn analysis(store: &SqliteStore) -> Vec<Stored> {
    store
        .db()
        .read_blocking(|conn| {
            conn.prepare(
                "SELECT path, analyzer_version, loudness_lufs, peak_dbtp, \
                 EXISTS (SELECT 1 FROM track_waveforms w WHERE w.track_id = t.id) \
                 FROM tracks t ORDER BY path",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect()
        })
        .unwrap()
}

/// The album's loudness, rounded to a tenth of a LUFS.
fn album_lufs(store: &SqliteStore) -> Option<f64> {
    scalar::<Option<f64>>(store, "SELECT loudness_lufs FROM albums WHERE title = 'X'")
        .unwrap()
        .map(|l| (l * 10.0).round() / 10.0)
}

/// A result that measured `lufs`, a peak of -1 dBTP, and a waveform.
fn measured(lufs: f64) -> AnalysisResult {
    AnalysisResult {
        integrated_lufs: Some(lufs),
        true_peak_dbtp: Some(-1.0),
        waveform: Waveform { peaks: vec![1, 2], rms: vec![1, 1] },
        ..AnalysisResult::empty(ANALYZER_VERSION)
    }
}

#[test]
fn replaced_audio_loses_its_analysis_and_a_lyrics_file_keeps_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 2);
    let (one, two) = (root.join("A/X/01 - Track 1.wav"), root.join("A/X/02 - Track 2.wav"));
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    for (track, file) in store.next_unanalyzed(lib.id, ANALYZER_VERSION, 2).unwrap() {
        let lufs = if file.path == one { -10.0 } else { -20.0 };
        store.store_analysis(lib.id, track, measured(lufs)).unwrap();
    }
    let version = i64::from(ANALYZER_VERSION);
    let analyzed = |path: &str, lufs| (path.to_owned(), version, Some(lufs), Some(-1.0), true);
    let first = analyzed("A/X/01 - Track 1.wav", -10.0);
    assert_eq!(analysis(&store), [first.clone(), analyzed("A/X/02 - Track 2.wav", -20.0)]);
    assert_eq!(album_lufs(&store), Some(-12.6));

    fs::write(one.with_extension("lrc"), "[00:01.00]hello\n").unwrap();
    assert_eq!(testing::scan_library(&store, &lib).progress.updated, 1);
    assert_eq!(analysis(&store), [first.clone(), analyzed("A/X/02 - Track 2.wav", -20.0)]);

    testing::replace_audio(&two, "Track 2");
    assert_eq!(testing::scan_library(&store, &lib).progress.updated, 1);
    let reset = ("A/X/02 - Track 2.wav".to_owned(), 0, None, None, false);
    assert_eq!(analysis(&store), [first, reset]);
    assert_eq!(album_lufs(&store), Some(-10.0), "the album is as loud as its analyzed track");
}

/// A result for audio replaced and rescanned during its decode measured the old audio: it
/// writes neither loudness nor waveform, and the track stays queued.
#[test]
fn a_result_for_replaced_audio_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    let (picked, file) = store.next_unanalyzed(lib.id, ANALYZER_VERSION, 1).unwrap().remove(0);

    testing::replace_audio(&file.path, "Track 1");
    assert_eq!(testing::scan_library(&store, &lib).progress.updated, 1);
    store.store_analysis(lib.id, picked, measured(-10.0)).unwrap();
    assert_eq!(analysis(&store), [("A/X/01 - Track 1.wav".to_owned(), 0, None, None, false)]);
    assert_eq!(store.next_unanalyzed(lib.id, ANALYZER_VERSION, 10).unwrap().len(), 1);
}

/// Audio replaced by a file that cannot be read loses its loudness, peak and waveform, though the
/// track stays, and is not queued until a scan reads the file. Its album is as loud as its other
/// track.
#[test]
fn audio_replaced_by_an_unreadable_file_loses_its_analysis() {
    let (_tmp, store, lib, root) = measured_album();
    testing::break_file(&root.join("A/X/02 - Track 2.wav"));
    let scan = testing::scan_library(&store, &lib);
    assert_eq!((scan.progress.problems, scan.progress.missing), (1, 0), "{:?}", scan.progress);
    let version = i64::from(ANALYZER_VERSION);
    let first = ("A/X/01 - Track 1.wav".to_owned(), version, Some(-10.0), Some(-1.0), true);
    let cleared = ("A/X/02 - Track 2.wav".to_owned(), 0, None, None, false);
    assert_eq!(analysis(&store), [first, cleared]);
    assert!(store.next_unanalyzed(lib.id, ANALYZER_VERSION, 10).unwrap().is_empty());
    assert_eq!(album_lufs(&store), Some(-10.0), "the album is as loud as its analyzed track");
}

/// A track measured as silence, with a waveform but no loudness or peak, loses its waveform too
/// when its file is replaced by one that cannot be read.
#[test]
fn a_silent_track_replaced_unreadably_loses_its_waveform() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    let silence = AnalysisResult {
        waveform: Waveform { peaks: vec![0, 0], rms: vec![0, 0] },
        ..AnalysisResult::empty(ANALYZER_VERSION)
    };
    let (track, file) = store.next_unanalyzed(lib.id, ANALYZER_VERSION, 1).unwrap().remove(0);
    store.store_analysis(lib.id, track, silence).unwrap();
    let version = i64::from(ANALYZER_VERSION);
    let path = "A/X/01 - Track 1.wav".to_owned();
    assert_eq!(analysis(&store), [(path.clone(), version, None, None, true)]);

    testing::break_file(&file.path);
    assert_eq!(testing::scan_library(&store, &lib).progress.problems, 1);
    assert_eq!(analysis(&store), [(path, 0, None, None, false)]);
}

/// A missing track whose file comes back unreadable with its size and modified time, read only
/// because a lyrics file appeared beside it, is back with its analysis, waveform and album
/// loudness as they were: its audio did not change.
#[test]
fn a_missing_file_back_unreadable_and_unchanged_keeps_its_analysis() {
    let (_tmp, store, lib, root) = measured_album();
    let two = root.join("A/X/02 - Track 2.wav");
    let before = analysis(&store);
    let (size, at) = (fs::metadata(&two).unwrap().len(), fs::metadata(&two).unwrap().modified());
    fs::remove_file(&two).unwrap();
    assert_eq!(testing::scan_library(&store, &lib).progress.missing, 1);

    fs::write(&two, vec![0; usize::try_from(size).unwrap()]).unwrap();
    fs::File::open(&two).unwrap().set_modified(at.unwrap()).unwrap();
    fs::write(two.with_extension("lrc"), "[00:01.00]hello\n").unwrap();
    let scan = testing::scan_library(&store, &lib);
    assert_eq!((scan.progress.problems, scan.progress.missing), (1, 0), "{:?}", scan.progress);
    assert_eq!(
        scalar::<i64>(&store, "SELECT COUNT(*) FROM tracks WHERE missing_since IS NULL"),
        Some(2)
    );
    assert_eq!(analysis(&store), before);
    assert_eq!(album_lufs(&store), Some(-12.6));
}

/// Two tracks under `A/X` scanned into a fresh store and measured at -10 and -20 LUFS, with the
/// store, library and root.
fn measured_album() -> (tempfile::TempDir, SqliteStore, LibraryConfig, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 2);
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    for (track, file) in store.next_unanalyzed(lib.id, ANALYZER_VERSION, 2).unwrap() {
        let lufs = if file.path.ends_with("01 - Track 1.wav") { -10.0 } else { -20.0 };
        store.store_analysis(lib.id, track, measured(lufs)).unwrap();
    }
    assert_eq!(album_lufs(&store), Some(-12.6));
    (tmp, store, lib, root)
}
