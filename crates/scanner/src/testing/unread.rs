//! Scenarios for a known file that is still there but can no longer be read: it keeps its track,
//! as a problem, rather than being marked missing, and is read again on every scan until it reads
//! (`design/scanning.md` §8).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::reanalysis::{analyzed_album, modified, set_modified};
use super::{Factory, InspectStore, library, live, make_album, scan_library, tag_wav};
use super::{InspectTrack, write_small_wav};
use crate::analysis::ANALYZER_VERSION;
use crate::store::Batch;
use crate::types::{AnalysisResult, LibraryConfig, ProblemKind, ScanState};

/// Overwrites the file at `path` with bytes that are not audio, and dates it five seconds after
/// it was last modified, so a scan finds it changed.
pub fn break_file(path: &Path) {
    let at = modified(path) + Duration::from_secs(5);
    fs::write(path, b"no longer audio").unwrap();
    set_modified(path, at);
}

/// Every recorded problem's path and kind, in path order.
fn problems(store: &dyn InspectStore, lib: &LibraryConfig) -> Vec<(PathBuf, ProblemKind)> {
    let mut found: Vec<_> =
        store.inspect_problems(lib.id).into_iter().map(|p| (p.path, p.kind)).collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// The live track at `path`.
fn track_at(store: &dyn InspectStore, lib: &LibraryConfig, path: &Path) -> InspectTrack {
    live(store, lib).into_iter().find(|t| t.path == path).unwrap()
}

/// `path` with a corrupt-audio problem.
fn corrupt(path: &Path) -> (PathBuf, ProblemKind) {
    (path.to_path_buf(), ProblemKind::CorruptAudio)
}

/// A known file replaced by one that cannot be read keeps its track, with its tags, as a problem,
/// while a new file that cannot be read gets only the problem. The track's analysis measured the
/// old file, so it is cleared, and the track is not queued, so analysis never opens the file.
/// Every scan reads both files again and counts their problems, writing nothing else. Once fixed,
/// the file is read into the same track, queued for analysis, and its problem clears.
pub fn a_known_file_that_cannot_be_read_keeps_its_track<S: InspectStore>(make: Factory<S>) {
    let (_tmp, store, lib, dir) = analyzed_album(make, 2);
    let (one, new) = (dir.join("01 - Track 1.wav"), dir.join("03 - New.wav"));
    let kept = live(&store, &lib)[0].clone();
    break_file(&one);
    fs::write(&new, b"not audio either").unwrap();

    let head = store.feed_head(lib.id);
    let scan = scan_library(&store, &lib);
    let p = &scan.progress;
    assert_eq!((scan.state, p.problems, p.missing), (ScanState::Completed, 2, 0), "{p:?}");
    assert_eq!((p.added, p.updated, p.files_processed), (0, 0, 2), "{p:?}");
    let tracks = live(&store, &lib);
    assert_eq!(tracks.len(), 2, "{tracks:?}");
    assert_eq!((tracks[0].id, &tracks[0].title), (kept.id, &kept.title));
    assert_eq!((tracks[0].measured, tracks[1].measured), (false, true));
    assert!(store.next_unanalyzed(lib.id, ANALYZER_VERSION, 10).unwrap().is_empty());
    assert_eq!(store.feed_tracks_after(lib.id, head), [kept.id], "the clear is announced");
    assert_eq!(problems(&store, &lib), [corrupt(&one), corrupt(&new)]);

    let head = store.feed_head(lib.id);
    let scan = scan_library(&store, &lib);
    let p = &scan.progress;
    assert_eq!((p.problems, p.missing, p.updated, p.files_processed), (2, 0, 0, 2), "{p:?}");
    assert_eq!(live(&store, &lib).len(), 2);
    assert_eq!(store.feed_head(lib.id), head, "nothing flaps");
    assert_eq!(problems(&store, &lib), [corrupt(&one), corrupt(&new)]);

    write_small_wav(&one);
    tag_wav(&one, "Track 1 again", "A", "X");
    let scan = scan_library(&store, &lib);
    assert_eq!((scan.progress.updated, scan.progress.problems), (1, 1), "{:?}", scan.progress);
    let fixed = track_at(&store, &lib, &one);
    assert_eq!((fixed.id, fixed.title.as_str(), fixed.analyzed), (kept.id, "Track 1 again", false));
    assert_eq!(problems(&store, &lib), [corrupt(&new)]);
}

/// A retagged file that cannot be opened is read again on every scan, even after a job that
/// opened it cleared the problem, and once it can be read again its new tags are taken and its
/// problem clears, though making it readable left its modified time alone. Skipped, saying so,
/// where permissions do not stop this user reading, as for root.
pub fn an_unreadable_file_is_read_again_until_it_reads<S: InspectStore>(make: Factory<S>) {
    let (_tmp, store, lib, dir) = analyzed_album(make, 1);
    let one = dir.join("01 - Track 1.wav");
    tag_wav(&one, "Retagged", "A", "X");
    let mode = |m| fs::set_permissions(&one, fs::Permissions::from_mode(m)).unwrap();
    mode(0o000);
    if fs::File::open(&one).is_ok() {
        mode(0o644);
        eprintln!("skipped: permissions do not stop this user reading a file (running as root?)");
        return;
    }
    let denied = || [(one.clone(), ProblemKind::PermissionDenied)];

    assert_eq!(scan_library(&store, &lib).progress.problems, 1);
    assert_eq!(problems(&store, &lib), denied());
    assert!(store.next_unanalyzed(lib.id, ANALYZER_VERSION, 10).unwrap().is_empty());
    store.apply(lib.id, Batch { opened: vec![one.clone()], ..Batch::default() }).unwrap();
    assert_eq!(problems(&store, &lib), []);
    assert_eq!(scan_library(&store, &lib).progress.problems, 1);
    assert_eq!(problems(&store, &lib), denied());

    let at = modified(&one);
    mode(0o644);
    assert_eq!(modified(&one), at);
    assert_eq!(scan_library(&store, &lib).progress.updated, 1);
    assert_eq!(track_at(&store, &lib, &one).title, "Retagged");
    assert_eq!(problems(&store, &lib), []);
}

/// A broken file moved out of the library loses its problem with its path, and moved back as it
/// was, its track returns with the problem again.
pub fn a_broken_file_moved_away_and_back_keeps_its_problem<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    let (one, away) = (root.join("A/X/01 - Track 1.wav"), tmp.path().join("away.wav"));
    break_file(&one);
    scan_library(&store, &lib);
    assert_eq!(problems(&store, &lib), [corrupt(&one)]);

    fs::rename(&one, &away).unwrap();
    assert_eq!(scan_library(&store, &lib).progress.missing, 1);
    assert_eq!(problems(&store, &lib), []);
    fs::rename(&away, &one).unwrap();
    assert_eq!(scan_library(&store, &lib).progress.problems, 1);
    assert_eq!(live(&store, &lib).len(), 1);
    assert_eq!(problems(&store, &lib), [corrupt(&one)]);
}

/// A file corrupted and then restored from a backup that kept its size and modified time, as
/// `cp -p` and `rsync -a` do, is read again: its problem clears and its track is queued for
/// analysis, since the measurements were cleared while it was broken.
pub fn a_file_restored_with_its_old_time_is_read_again<S: InspectStore>(make: Factory<S>) {
    let (_tmp, store, lib, dir) = analyzed_album(make, 1);
    let one = dir.join("01 - Track 1.wav");
    let (bytes, at) = (fs::read(&one).unwrap(), modified(&one));
    break_file(&one);
    assert_eq!(scan_library(&store, &lib).progress.problems, 1);

    fs::write(&one, &bytes).unwrap();
    set_modified(&one, at);
    let scan = scan_library(&store, &lib);
    assert_eq!((scan.progress.updated, scan.progress.problems), (1, 0), "{:?}", scan.progress);
    assert_eq!(problems(&store, &lib), []);
    let queued = store.next_unanalyzed(lib.id, ANALYZER_VERSION, 10).unwrap();
    assert_eq!(queued.into_iter().map(|(_, file)| file.path).collect::<Vec<_>>(), [one]);
}

/// A result for a file a scan found changed and unreadable during its decode is not stored, even
/// though the file kept its size: it measured audio that is gone. The queue holds only the track
/// beside it, whose file is fine, until a scan reads the broken file.
pub fn a_result_for_a_file_broken_during_its_decode_is_not_stored<S: InspectStore>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    let (one, two) = (root.join("A/X/01 - Track 1.wav"), root.join("A/X/02 - Track 2.wav"));
    let picked = store.next_unanalyzed(lib.id, ANALYZER_VERSION, 2).unwrap();
    let (size, at) = (fs::metadata(&one).unwrap().len(), modified(&one));
    fs::write(&one, vec![0; usize::try_from(size).unwrap()]).unwrap();
    set_modified(&one, at + Duration::from_secs(5));
    assert_eq!(scan_library(&store, &lib).progress.problems, 1);
    let queued = store.next_unanalyzed(lib.id, ANALYZER_VERSION, 10).unwrap();
    assert_eq!(queued.into_iter().map(|(_, file)| file.path).collect::<Vec<_>>(), [two]);

    let measured =
        AnalysisResult { integrated_lufs: Some(-10.0), ..AnalysisResult::empty(ANALYZER_VERSION) };
    for (track, _) in picked {
        store.store_analysis(lib.id, track, measured.clone()).unwrap();
    }
    let tracks = live(&store, &lib);
    assert_eq!((tracks[0].analyzed, tracks[0].measured, tracks[1].measured), (false, false, true));
}

/// A known file replaced by one that cannot be read loses its analysis whether only its modified
/// time changed, as for other bytes of the same length, or only its size, the time put back.
pub fn an_unreadable_replacement_loses_its_analysis<S: InspectStore>(make: Factory<S>) {
    let (_tmp, store, lib, dir) = analyzed_album(make, 3);
    let track = |i: usize| dir.join(format!("{i:02} - Track {i}.wav"));
    let size = fs::metadata(track(1)).unwrap().len();
    let at = modified(&track(1)) + Duration::from_secs(5);
    fs::write(track(1), vec![0; usize::try_from(size).unwrap()]).unwrap();
    set_modified(&track(1), at);
    let at = modified(&track(2));
    fs::write(track(2), b"no longer audio").unwrap();
    set_modified(&track(2), at);

    assert_eq!(scan_library(&store, &lib).progress.problems, 2);
    let measured: Vec<bool> = live(&store, &lib).iter().map(|t| t.measured).collect();
    assert_eq!(measured, [false, false, true]);
    assert!(store.next_unanalyzed(lib.id, ANALYZER_VERSION, 10).unwrap().is_empty());
}

/// A missing track whose file comes back unreadable is back, as a problem, since its file is
/// there. One that changed loses its analysis; one that kept its size and modified time, read only
/// because a lyrics file appeared beside it, keeps it.
pub fn a_missing_file_back_unreadable_is_back<S: InspectStore>(make: Factory<S>) {
    let (_tmp, store, lib, dir) = analyzed_album(make, 3);
    let (two, three) = (dir.join("02 - Track 2.wav"), dir.join("03 - Track 3.wav"));
    let (size, at) = (fs::metadata(&three).unwrap().len(), modified(&three));
    fs::remove_file(&two).unwrap();
    fs::remove_file(&three).unwrap();
    assert_eq!(scan_library(&store, &lib).progress.missing, 2);
    let mut gone: Vec<_> =
        store.inspect_tracks(lib.id).into_iter().filter(|t| t.missing).map(|t| t.id).collect();
    gone.sort();

    let head = store.feed_head(lib.id);
    fs::write(&two, b"no longer audio").unwrap();
    fs::write(&three, vec![0; usize::try_from(size).unwrap()]).unwrap();
    set_modified(&three, at);
    fs::write(three.with_extension("lrc"), "[00:01.00]hello\n").unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!((scan.progress.problems, scan.progress.added), (2, 0), "{:?}", scan.progress);
    let measured: Vec<bool> = live(&store, &lib).iter().map(|t| t.measured).collect();
    assert_eq!(measured, [true, false, true]);
    let mut announced = store.feed_tracks_after(lib.id, head);
    announced.sort();
    assert_eq!(announced, gone, "the returns are announced");
    assert_eq!(problems(&store, &lib), [corrupt(&two), corrupt(&three)]);
}
