//! Scenarios for the background decodes: what a failure leaves behind (`design/scanning.md` §12).

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use jewelcase_ffmpeg::{Config, Ffmpeg};

use super::{Factory, InspectStore, PNG, library, make_album, scan_library};
use crate::analysis::{ANALYZER_VERSION, AnalysisResult, Analyzer};
use crate::clock::now_ms;
use crate::jobs::RunReport;
use crate::library::LibraryId;
use crate::placeholders::Placeholders;
use crate::problems::{Problem, ProblemKind};

/// An analyzer over `store` that runs ffmpeg with `config`.
fn analyzer<S: InspectStore + 'static>(store: &Arc<S>, config: Config) -> Analyzer {
    Analyzer::new(Ffmpeg::new(config), store.clone())
}

/// Whether the track at `path` has an analysis result.
fn analyzed(store: &impl InspectStore, lib: LibraryId, path: &Path) -> bool {
    let tracks = store.inspect_tracks(lib);
    tracks.iter().find(|t| t.path == path).unwrap_or_else(|| panic!("{tracks:?}")).analyzed
}

/// A pass that did not back off, with these counts.
fn report(done: usize, failed: usize, skipped: usize) -> RunReport {
    RunReport { done, failed, skipped, backed_off: false }
}

/// A pass that backed off having done nothing.
const BACKED_OFF: RunReport = RunReport { done: 0, failed: 0, skipped: 0, backed_off: true };

/// A two-track album, scanned into a fresh store, with the path of its first track.
fn album<S: InspectStore>(make: Factory<S>) -> (tempfile::TempDir, Arc<S>, LibraryId, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    scan_library(store.as_ref(), &lib);
    (tmp, store, lib.id, root.join("A/X/01 - Track 1.wav"))
}

/// The first track, made unopenable by `hide` and openable again by `show`, gets no result and
/// a `kind` problem. It is tried again once its wait is over, skipped while it waits however many
/// passes that lasts, and analyzed once it opens, even after a restart, which clears the problem.
fn waits_until_it_opens<S: InspectStore + 'static>(
    make: Factory<S>,
    hide: fn(&Path),
    show: fn(&Path),
    kind: ProblemKind,
) {
    let (_tmp, store, lib, file) = album(make);
    let problem = |store: &S| store.inspect_problems(lib).into_iter().map(|p| (p.path, p.kind));
    hide(&file);

    let retried = analyzer(&store, Config::default()).with_retry_after(Duration::ZERO);
    assert_eq!(retried.run_once(lib, 10), report(1, 0, 1), "the other track goes on");
    assert_eq!(problem(&store).collect::<Vec<_>>(), [(file.clone(), kind)]);
    assert_eq!(retried.run_once(lib, 10), report(0, 0, 1), "tried again after its wait");

    let waiting = analyzer(&store, Config::default());
    assert_eq!(waiting.run_once(lib, 10), report(0, 0, 1));
    assert_eq!(waiting.run_once(lib, 10), RunReport::default());
    show(&file);
    assert_eq!(waiting.run_once(lib, 10), RunReport::default(), "still waiting");
    assert!(!analyzed(store.as_ref(), lib, &file));

    let restarted = analyzer(&store, Config::default());
    assert_eq!(restarted.run_once(lib, 10), report(1, 0, 0));
    assert!(analyzed(store.as_ref(), lib, &file));
    assert_eq!(problem(&store).count(), 0);
}

/// A file moved away after the scan, under a root that is still there, gets no result until it
/// returns.
pub fn a_missing_file_waits_until_it_returns<S: InspectStore + 'static>(make: Factory<S>) {
    fn away(file: &Path) -> PathBuf {
        file.with_extension("away")
    }
    let hide = |file: &Path| fs::rename(file, away(file)).unwrap();
    let show = |file: &Path| fs::rename(away(file), file).unwrap();
    waits_until_it_opens(make, hide, show, ProblemKind::Unreadable);
}

/// A file that may not be read gets no result until it may.
pub fn a_file_not_permitted_waits_until_it_is<S: InspectStore + 'static>(make: Factory<S>) {
    let probe = tempfile::NamedTempFile::new().unwrap();
    fs::set_permissions(probe.path(), fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(probe.path()).is_ok() {
        eprintln!("skipping: permissions do not apply to this user");
        return;
    }
    let hide = |file: &Path| fs::set_permissions(file, fs::Permissions::from_mode(0o000)).unwrap();
    let show = |file: &Path| fs::set_permissions(file, fs::Permissions::from_mode(0o644)).unwrap();
    waits_until_it_opens(make, hide, show, ProblemKind::PermissionDenied);
}

/// A root that goes away while the queue is being worked through stops analysis and
/// placeholders without writing anything, however many passes it lasts.
pub fn a_root_going_away_pauses_without_writing<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 3);
    write_cover(&root.join("A/X/cover.png"));
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    scan_library(store.as_ref(), &lib);
    let analyzer = analyzer(&store, Config::default());
    let placeholders = Placeholders::new(Ffmpeg::new(Config::default()), store.clone());
    assert_eq!(analyzer.run_once(lib.id, 1), report(1, 0, 0));

    let away = tmp.path().join("away");
    fs::rename(&root, &away).unwrap();
    for _ in 0..3 {
        assert_eq!(analyzer.run_once(lib.id, 10), BACKED_OFF);
        assert_eq!(placeholders.run_once(lib.id, 10), BACKED_OFF);
    }
    let tracks = store.inspect_tracks(lib.id);
    assert_eq!(tracks.iter().filter(|t| t.analyzed).count(), 1, "{tracks:?}");
    assert_eq!(store.next_without_placeholder(lib.id, 10).unwrap().len(), 1);
    assert!(store.inspect_problems(lib.id).is_empty());

    fs::rename(&away, &root).unwrap();
    assert_eq!(analyzer.run_once(lib.id, 10), report(2, 0, 0));
    assert_eq!(placeholders.run_once(lib.id, 10), report(1, 0, 0));
}

/// A file whose every probe is killed by a signal backs the worker off once, then is judged and
/// recorded as a problem, so the track after it is still analyzed.
pub fn a_file_that_always_fails_does_not_block_the_next<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    let killed = root.join("A/X/killed.wav");
    fs::rename(root.join("A/X/01 - Track 1.wav"), &killed).unwrap();
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    scan_library(store.as_ref(), &lib);
    let ffprobe = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-ffprobe.sh");
    let analyzer = analyzer(&store, Config { ffprobe, ..Config::default() });

    assert!(analyzer.run_once(lib.id, 10).backed_off);
    assert!(!analyzed(store.as_ref(), lib.id, &killed), "a first failure may pass");
    assert_eq!(analyzer.run_once(lib.id, 10).failed, 1);
    assert_eq!(analyzer.run_once(lib.id, 10), RunReport::default(), "nothing left");
    let tracks = store.inspect_tracks(lib.id);
    assert!(tracks.iter().all(|t| t.analyzed), "{tracks:?}");
    let problems = store.inspect_problems(lib.id);
    assert_eq!(problems.iter().map(|p| &p.path).collect::<Vec<_>>(), [&killed]);
    assert!(problems[0].detail.starts_with("ffmpeg was killed by signal 9"), "{problems:?}");
}

/// A track never analyzed is at version 0, and results from an older analyzer leave a track in
/// the queue for a newer one (`design/scanning.md` §12).
pub fn an_older_analyzers_results_are_redone<S: InspectStore>(make: Factory<S>) {
    let (_tmp, store, lib, _) = album(make);
    let queued = |version| store.next_unanalyzed(lib, version, 10).unwrap().len();
    assert_eq!((queued(0), queued(1)), (0, 2), "nothing is below version 0");
    let track = store.next_unanalyzed(lib, 1, 1).unwrap()[0].0;
    store.store_analysis(lib, track, AnalysisResult::empty(1)).unwrap();
    assert_eq!((queued(1), queued(2)), (1, 2), "version 1's result is redone for version 2");
}

/// A file that cannot be opened, first in the queue, does not hold up a pass of one once it
/// waits: the next pass moves on to the track after it.
pub fn an_unopenable_file_does_not_starve_the_next<S: InspectStore + 'static>(make: Factory<S>) {
    let (_tmp, store, lib, _) = album(make);
    // The queue's order is the store's to choose.
    let first = store.next_unanalyzed(lib, ANALYZER_VERSION, 1).unwrap().remove(0).1.path;
    fs::rename(&first, first.with_extension("away")).unwrap();
    let analyzer = analyzer(&store, Config::default());
    assert_eq!(analyzer.run_once(lib, 1), report(0, 0, 1));
    assert_eq!(analyzer.run_once(lib, 1), report(1, 0, 0));
}

/// Opening a file clears only a problem saying it could not be opened: a placeholder made from a
/// track's picture leaves the track's analysis verdict.
pub fn opening_a_file_clears_only_unopened_problems<S: InspectStore + 'static>(make: Factory<S>) {
    let (_tmp, store, lib, file) = album(make);
    let other = file.with_file_name("02 - Track 2.wav");
    let problem = |path: &Path, kind| Problem {
        path: path.to_path_buf(),
        kind,
        detail: "detail".into(),
        seen_at: now_ms(),
    };
    store.record_problem(lib, problem(&file, ProblemKind::CorruptAudio)).unwrap();
    store.record_problem(lib, problem(&other, ProblemKind::Unreadable)).unwrap();
    store.clear_unopened(lib, &file).unwrap();
    store.clear_unopened(lib, &other).unwrap();
    let left = store.inspect_problems(lib);
    assert_eq!(
        left.iter().map(|p| (&p.path, p.kind)).collect::<Vec<_>>(),
        [(&file, ProblemKind::CorruptAudio)]
    );
}

/// A cover moved away after the scan gets no placeholder and a problem, and a placeholder once
/// it returns.
pub fn a_cover_moved_away_waits_until_it_returns<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let cover = root.join("A/X/cover.png");
    write_cover(&cover);
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    scan_library(store.as_ref(), &lib);
    let hash = store.next_without_placeholder(lib.id, 10).unwrap().remove(0).0;
    let placeholders = Placeholders::new(Ffmpeg::new(Config::default()), store.clone())
        .with_retry_after(Duration::ZERO);

    fs::rename(&cover, tmp.path().join("cover.png")).unwrap();
    assert_eq!(placeholders.run_once(lib.id, 10), report(0, 0, 1));
    let problems = store.inspect_problems(lib.id);
    assert_eq!(
        problems.iter().map(|p| (&p.path, p.kind)).collect::<Vec<_>>(),
        [(&cover, ProblemKind::Unreadable)]
    );
    assert_eq!(store.inspect_placeholder(lib.id, &hash), None);

    fs::rename(tmp.path().join("cover.png"), &cover).unwrap();
    assert_eq!(placeholders.run_once(lib.id, 10), report(1, 0, 0));
    assert!(store.inspect_placeholder(lib.id, &hash).is_some_and(|p| !p.is_empty()));
    assert!(store.inspect_problems(lib.id).is_empty());
}

/// A track replaced by a pipe after the scan is judged without opening it, which would block
/// the worker, and its shutdown, until something wrote to the pipe.
pub fn a_pipe_is_judged_without_blocking<S: InspectStore + 'static>(make: Factory<S>) {
    let (_tmp, store, lib, file) = album(make);
    fs::remove_file(&file).unwrap();
    assert!(Command::new("mkfifo").arg(&file).status().unwrap().success());
    let analyzer = analyzer(&store, Config::default());
    let (sent, received) = mpsc::channel();
    std::thread::spawn(move || sent.send(analyzer.run_once(lib, 10)));
    let report = received.recv_timeout(Duration::from_secs(30)).expect("the pass blocked");
    assert_eq!(report, self::report(1, 1, 0));
    let problems = store.inspect_problems(lib);
    assert_eq!(
        problems.iter().map(|p| (&p.path, p.detail.as_str())).collect::<Vec<_>>(),
        [(&file, "not a regular file")]
    );
}

/// Writes a small red PNG to `path`.
fn write_cover(path: &Path) {
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=red:s=8x8", "-frames:v", "1"])
        .arg(path)
        .status();
    assert!(status.unwrap().success());
}

/// A decoder that cannot start says nothing about any file: every pass backs off.
pub fn a_decoder_that_cannot_start_backs_off<S: InspectStore + 'static>(make: Factory<S>) {
    let (_tmp, store, lib, _) = album(make);
    let ffprobe = PathBuf::from("/nonexistent/ffprobe");
    let analyzer = analyzer(&store, Config { ffprobe, ..Config::default() });
    for _ in 0..3 {
        assert_eq!(analyzer.run_once(lib, 10), BACKED_OFF);
    }
    assert!(store.inspect_tracks(lib).iter().all(|t| !t.analyzed));
    assert!(store.inspect_problems(lib).is_empty());
}

/// A file that becomes a symlink after the scan is never decoded through it, whether the link
/// leads inside the library, out of it, or nowhere: it is judged at once and recorded.
pub fn a_link_is_judged_once<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 4);
    fs::write(root.join("A/X/cover.png"), PNG).unwrap();
    let lib = library(&root);
    let store = Arc::new(make(&lib));
    scan_library(store.as_ref(), &lib);
    let mut links = [
        (root.join("A/X/01 - Track 1.wav"), root.join("moved.wav")),
        (root.join("A/X/02 - Track 2.wav"), tmp.path().join("outside.wav")),
        (root.join("A/X/03 - Track 3.wav"), tmp.path().join("gone.wav")),
        (root.join("A/X/cover.png"), tmp.path().join("cover.png")),
    ];
    for (link, target) in &links {
        fs::rename(link, target).unwrap();
        symlink(target, link).unwrap();
    }
    fs::remove_file(tmp.path().join("gone.wav")).unwrap();
    let analyzer = analyzer(&store, Config::default());
    let placeholders = Placeholders::new(Ffmpeg::new(Config::default()), store.clone());

    assert_eq!(analyzer.run_once(lib.id, 10), report(1, 3, 0));
    assert_eq!(analyzer.run_once(lib.id, 10), RunReport::default(), "not tried again");
    assert_eq!(placeholders.run_once(lib.id, 10), report(0, 1, 0));
    assert_eq!(placeholders.run_once(lib.id, 10), RunReport::default(), "not tried again");
    let problems = store.inspect_problems(lib.id);
    assert!(problems.iter().all(|p| p.detail == "symlinks are not followed"), "{problems:?}");
    let mut paths: Vec<_> = problems.into_iter().map(|p| p.path).collect();
    paths.sort();
    links.sort();
    assert_eq!(paths, links.map(|(link, _)| link));
}
