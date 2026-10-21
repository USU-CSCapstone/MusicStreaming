//! Scenarios for when a rescan keeps a track's analysis and when it queues the track again
//! (`design/scanning.md` §12).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{
    Factory, InspectStore, library, live, make_album, scan_library, tag_wav, write_sine_wav,
};
use crate::analysis::{ANALYZER_VERSION, AnalysisResult};
use crate::library::LibraryConfig;

/// An album of `n` tracks under `A/X`, scanned into a fresh store with every track analyzed and
/// its loudness measured, and the album's folder.
pub(super) fn analyzed_album<S: InspectStore>(
    make: Factory<S>,
    n: usize,
) -> (tempfile::TempDir, S, LibraryConfig, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", n);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    for (track, _) in store.next_unanalyzed(lib.id, ANALYZER_VERSION, n).unwrap() {
        let measured = AnalysisResult {
            integrated_lufs: Some(-10.0),
            ..AnalysisResult::empty(ANALYZER_VERSION)
        };
        store.store_analysis(lib.id, track, measured).unwrap();
    }
    assert!(live(&store, &lib).iter().all(|t| t.measured));
    (tmp, store, lib, root.join("A/X"))
}

/// When `path` was last modified.
pub(super) fn modified(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

/// Sets when `path` was last modified to `at`.
pub(super) fn set_modified(path: &Path, at: SystemTime) {
    fs::File::open(path).unwrap().set_modified(at).unwrap();
}

/// Replaces the audio of the track `make_album` made at `path`, titled `title`, with other audio
/// of the same length and tags, so only its modified time changes.
pub fn replace_audio(path: &Path, title: &str) {
    let at = modified(path) + Duration::from_secs(5);
    write_sine_wav(path, 880.0, 0.5, 0.2);
    tag_wav(path, title, "A", "X");
    set_modified(path, at);
}

/// A file whose size or modified time changed is queued for analysis again, since neither says
/// whether its audio or only its tags changed: replaced audio of the same length (the time
/// alone), tags rewritten with the time put back (the size alone), and a retag (both). An
/// untouched file keeps its result.
pub fn a_changed_file_is_analyzed_again<S: InspectStore>(make: Factory<S>) {
    let (_tmp, store, lib, dir) = analyzed_album(make, 4);
    let track = |i: usize| dir.join(format!("{i:02} - Track {i}.wav"));
    let size = |i: usize| fs::metadata(track(i)).unwrap().len();

    let size_1 = size(1);
    replace_audio(&track(1), "Track 1");
    assert_eq!(size(1), size_1, "the replaced audio is the same size");

    let (size_2, at_2) = (size(2), modified(&track(2)));
    tag_wav(&track(2), "Track Two", "A", "X");
    set_modified(&track(2), at_2);
    assert_ne!(size(2), size_2, "the retag changed the size");

    let at_3 = modified(&track(3)) + Duration::from_secs(5);
    tag_wav(&track(3), "Track Three", "A", "X");
    set_modified(&track(3), at_3);

    assert_eq!(scan_library(&store, &lib).progress.updated, 3);
    let analyzed: Vec<bool> = live(&store, &lib).iter().map(|t| t.analyzed).collect();
    assert_eq!(analyzed, [false, false, false, true]);
    let mut queued: Vec<PathBuf> = store
        .next_unanalyzed(lib.id, ANALYZER_VERSION, 10)
        .unwrap()
        .into_iter()
        .map(|(_, file)| file.path)
        .collect();
    queued.sort();
    assert_eq!(queued, [track(1), track(2), track(3)]);
}

/// A lyrics file written beside a track whose audio did not change, as a plugin does, keeps the
/// track's analysis: lyrics coming in must not queue the library for another decode.
pub fn a_lyrics_file_alone_keeps_the_analysis<S: InspectStore>(make: Factory<S>) {
    let (_tmp, store, lib, dir) = analyzed_album(make, 1);
    fs::write(dir.join("01 - Track 1.lrc"), "[00:01.00]hello\n").unwrap();

    assert_eq!(scan_library(&store, &lib).progress.updated, 1);
    let tracks = live(&store, &lib);
    assert!(tracks[0].has_sidecar_lyrics && tracks[0].analyzed, "{tracks:?}");
    assert!(store.next_unanalyzed(lib.id, ANALYZER_VERSION, 10).unwrap().is_empty());
}

/// A result for a file changed and rescanned while it was being decoded is not stored, whether
/// its audio was replaced (the modified time alone) or its tags rewritten with the time put back
/// (the size alone): it measured the old file, so the track stays queued for the new.
pub fn a_result_for_replaced_audio_is_not_stored<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    let picked = store.next_unanalyzed(lib.id, ANALYZER_VERSION, 2).unwrap();
    let (one, two) = (root.join("A/X/01 - Track 1.wav"), root.join("A/X/02 - Track 2.wav"));

    replace_audio(&one, "Track 1");
    let at = modified(&two);
    tag_wav(&two, "Track Two", "A", "X");
    set_modified(&two, at);
    assert_eq!(scan_library(&store, &lib).progress.updated, 2);
    for (track, _) in picked {
        store.store_analysis(lib.id, track, AnalysisResult::empty(ANALYZER_VERSION)).unwrap();
    }
    assert!(live(&store, &lib).iter().all(|t| !t.analyzed));
    assert_eq!(store.next_unanalyzed(lib.id, ANALYZER_VERSION, 10).unwrap().len(), 2);
}
