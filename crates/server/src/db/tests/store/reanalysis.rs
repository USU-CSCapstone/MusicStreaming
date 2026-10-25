//! What a rescan leaves of a track's analysis: its loudness and peak, its waveform, and the
//! loudness of its album.

use std::fs;

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
        waveform: vec![1, 1],
        ..AnalysisResult::empty(ANALYZER_VERSION)
    }
}

#[test]
fn replaced_audio_loses_its_analysis_and_a_lyrics_file_keeps_it() {
    let (_tmp, store, lib, root) = scanned_album(2);
    let (one, two) = (root.join("A/X/01 - Track 1.wav"), root.join("A/X/02 - Track 2.wav"));
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
    let (_tmp, store, lib, _) = scanned_album(1);
    let (picked, file) = store.next_unanalyzed(lib.id, ANALYZER_VERSION, 1).unwrap().remove(0);

    testing::replace_audio(&file.path, "Track 1");
    assert_eq!(testing::scan_library(&store, &lib).progress.updated, 1);
    store.store_analysis(lib.id, picked, measured(-10.0)).unwrap();
    assert_eq!(analysis(&store), [("A/X/01 - Track 1.wav".to_owned(), 0, None, None, false)]);
    assert_eq!(store.next_unanalyzed(lib.id, ANALYZER_VERSION, 10).unwrap().len(), 1);
}

/// Tracks analyzed by versions 1 and 2, whose waveforms this server does not read (linear values
/// in version 1, peaks beside RMS in version 2), are analyzed again.
#[test]
fn tracks_analyzed_by_versions_1_and_2_are_analyzed_again() {
    let (_tmp, store, lib, _) = scanned_album(2);
    let picked = store.next_unanalyzed(lib.id, ANALYZER_VERSION, 2).unwrap();
    assert_eq!(picked.len(), 2);
    for ((track, _), version) in picked.into_iter().zip([1, 2]) {
        store.store_analysis(lib.id, track, AnalysisResult::empty(version)).unwrap();
    }
    assert_eq!(store.next_unanalyzed(lib.id, ANALYZER_VERSION, 10).unwrap().len(), 2);
}

/// A result with no waveform, as a file judged broken gets, deletes the waveform its track had.
#[test]
fn a_result_without_a_waveform_deletes_the_old_one() {
    let (_tmp, store, lib, _) = scanned_album(1);
    let pick = || store.next_unanalyzed(lib.id, ANALYZER_VERSION, 1).unwrap().remove(0).0;
    store.store_analysis(lib.id, pick(), measured(-9.0)).unwrap();
    assert!(analysis(&store)[0].4, "measured with a waveform");

    let requeue = "UPDATE tracks SET analyzer_version = 0";
    store.db().write_blocking(move |tx| tx.execute(requeue, [])).unwrap();
    store.store_analysis(lib.id, pick(), AnalysisResult::empty(ANALYZER_VERSION)).unwrap();
    let version = i64::from(ANALYZER_VERSION);
    assert_eq!(analysis(&store), [("A/X/01 - Track 1.wav".to_owned(), version, None, None, false)]);
}

/// An album of `n` tracks under `A/X` scanned into a fresh store, with the store, library and
/// root.
fn scanned_album(n: usize) -> (tempfile::TempDir, SqliteStore, LibraryConfig, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", n);
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    (tmp, store, lib, root)
}
