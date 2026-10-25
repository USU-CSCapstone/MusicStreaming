//! Scenarios for a known file that is still there but can no longer be read: it keeps its track,
//! as a problem, rather than being marked missing, and is read again on every scan until it reads
//! (`design/scanning.md` §8).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::reanalysis::{analyzed_album, modified, set_modified};
use super::{Factory, InspectStore, live, scan_library, tag_wav};
use super::{InspectTrack, write_small_wav};

use crate::library::LibraryConfig;
use crate::problems::ProblemKind;
use crate::scan::ScanState;

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
/// while a new file that cannot be read gets only the problem. Nothing else is written, so a
/// scan with the file still broken records the same problems again. Once fixed, the file is read
/// into the same track and its problem clears.
pub fn a_known_file_that_cannot_be_read_keeps_its_track<S: InspectStore>(make: Factory<S>) {
    let (_tmp, store, lib, dir) = analyzed_album(make, 2);
    let (one, new) = (dir.join("01 - Track 1.wav"), dir.join("03 - New.wav"));
    let kept = live(&store, &lib)[0].clone();
    break_file(&one);
    fs::write(&new, b"not audio either").unwrap();

    for _ in 0..2 {
        let scan = scan_library(&store, &lib);
        let p = &scan.progress;
        assert_eq!((scan.state, p.problems, p.missing), (ScanState::Completed, 2, 0), "{p:?}");
        assert_eq!((p.added, p.updated, p.files_processed), (0, 0, 2), "{p:?}");
        let tracks = live(&store, &lib);
        assert_eq!(tracks.len(), 2, "{tracks:?}");
        assert_eq!((tracks[0].id, &tracks[0].title), (kept.id, &kept.title));
        assert_eq!(problems(&store, &lib), [corrupt(&one), corrupt(&new)]);
    }

    write_small_wav(&one);
    tag_wav(&one, "Track 1 again", "A", "X");
    let scan = scan_library(&store, &lib);
    assert_eq!((scan.progress.updated, scan.progress.problems), (1, 1), "{:?}", scan.progress);
    let fixed = track_at(&store, &lib, &one);
    assert_eq!((fixed.id, fixed.title.as_str()), (kept.id, "Track 1 again"));
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
    store.clear_unopened(lib.id, &one).unwrap();
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
