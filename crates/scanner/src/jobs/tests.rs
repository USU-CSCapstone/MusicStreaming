use std::fs;

use super::*;
use crate::clock::now_ms;
use crate::problems::ProblemKind;
use crate::testing::BareStore;

/// A root holding the files `a` and `b`, and a store to record problems in.
pub(super) fn setup() -> (tempfile::TempDir, BareStore, [FileToDecode; 2]) {
    let tmp = tempfile::tempdir().unwrap();
    let file = |name: &str| {
        fs::write(tmp.path().join(name), b"x").unwrap();
        FileToDecode { root: tmp.path().to_path_buf(), path: tmp.path().join(name) }
    };
    let files = [file("a"), file("b")];
    (tmp, BareStore::default(), files)
}

/// A stall decoding `file`: a failure that may pass.
fn stall(file: &FileToDecode) -> Failure {
    Failure::Attempt(Problem {
        path: file.path.clone(),
        kind: ProblemKind::Stalled,
        detail: "stalled".into(),
        seen_at: now_ms(),
    })
}

/// A file's second failure that may pass is a verdict, whatever other files failed or
/// decoded between, as they do when passes run at once; the verdict starts its count again.
#[test]
fn a_files_second_failure_is_a_verdict() {
    let (_tmp, store, [a, b]) = setup();
    let retries = Retries::new(RETRY_AFTER);
    let lib = 1;
    assert_eq!(retries.failed(&store, lib, &a, stall(&a)).unwrap(), Outcome::BackOff);
    assert_eq!(retries.failed(&store, lib, &b, stall(&b)).unwrap(), Outcome::BackOff);
    retries.succeeded(&store, lib, &b).unwrap();
    assert!(store.problems().is_empty());
    assert_eq!(retries.failed(&store, lib, &a, stall(&a)).unwrap(), Outcome::Verdict);
    assert_eq!(store.problems().len(), 1);
    assert_eq!(retries.failed(&store, lib, &a, stall(&a)).unwrap(), Outcome::BackOff);
    assert_eq!(retries.failed(&store, lib, &b, stall(&b)).unwrap(), Outcome::BackOff);
}

#[test]
fn a_success_resets_the_count() {
    let (_tmp, store, [a, _]) = setup();
    let retries = Retries::new(RETRY_AFTER);
    let lib = 1;
    assert_eq!(retries.failed(&store, lib, &a, stall(&a)).unwrap(), Outcome::BackOff);
    retries.succeeded(&store, lib, &a).unwrap();
    assert_eq!(retries.failed(&store, lib, &a, stall(&a)).unwrap(), Outcome::BackOff);
    assert!(store.problems().is_empty());
}

#[test]
fn a_failure_under_an_unavailable_root_backs_off_unrecorded() {
    let (tmp, store, [a, _]) = setup();
    let retries = Retries::new(RETRY_AFTER);
    let lib = 1;
    fs::rename(tmp.path(), tmp.path().with_extension("away")).unwrap();
    for _ in 0..3 {
        let gone = Failure::Open(io::ErrorKind::NotFound.into());
        assert_eq!(retries.failed(&store, lib, &a, gone).unwrap(), Outcome::BackOff);
        assert_eq!(retries.failed(&store, lib, &a, stall(&a)).unwrap(), Outcome::BackOff);
    }
    assert!(store.problems().is_empty());
    assert!(!retries.waiting(&a));
    fs::rename(tmp.path().with_extension("away"), tmp.path()).unwrap();
}

#[test]
fn a_decoder_error_on_a_file_that_went_away_is_no_verdict() {
    let (_tmp, store, [a, _]) = setup();
    let retries = Retries::new(RETRY_AFTER);
    let lib = 1;
    fs::remove_file(&a.path).unwrap();
    let rejected = jewelcase_ffmpeg::Error::Failed { status: Some(1), stderr: String::new() };
    let failure = Failure::from_ffmpeg(&a.path, &rejected);
    assert_eq!(retries.failed(&store, lib, &a, failure).unwrap(), Outcome::Skipped);
    assert!(retries.waiting(&a));
    assert_eq!(store.problems()[0].kind, ProblemKind::Unreadable);
}

#[test]
fn files_that_cannot_be_opened_do_not_count_towards_the_limit() {
    let (tmp, store, [a, b]) = setup();
    let retries = Retries::new(RETRY_AFTER);
    let lib = 1;
    let gone = FileToDecode { root: tmp.path().to_path_buf(), path: tmp.path().join("gone") };
    let queue = [(1, gone), (2, a.clone()), (3, b)];
    let next =
        |files: &[(usize, FileToDecode)], fetch| Ok(files.iter().take(fetch).cloned().collect());
    fs::remove_file(&a.path).unwrap();
    let decode = |file: &FileToDecode| file.open().map_err(Failure::Open);
    let keep = |_, _| Ok(());
    let report = retries.pass(&store, lib, 1, |n| next(&queue, n), decode, keep).unwrap();
    assert_eq!(report, RunReport { skipped: 1, ..RunReport::default() });
    // The file waiting leaves room for itself and one more found unopenable, then one decoded.
    let report = retries.pass(&store, lib, 1, |n| next(&queue, n), decode, keep).unwrap();
    assert_eq!(report, RunReport { done: 1, skipped: 1, ..RunReport::default() });
    // Two files wait, with room for as many more found unopenable.
    assert_eq!(retries.fetch(1, 0), 5);
    let report = retries.pass(&store, lib, 1, |n| next(&queue[..2], n), decode, keep).unwrap();
    assert_eq!(report, RunReport::default());
}

#[test]
fn a_file_whose_wait_is_over_is_forgotten() {
    let (_tmp, store, [a, _]) = setup();
    let retries = Retries::new(Duration::ZERO);
    let lib = 1;
    fs::remove_file(&a.path).unwrap();
    let gone = Failure::Open(io::ErrorKind::NotFound.into());
    assert_eq!(retries.failed(&store, lib, &a, gone).unwrap(), Outcome::Skipped);
    assert_eq!(retries.fetch(1, 0), 1);
    assert!(retries.waiting.lock().unwrap().is_empty());
}

#[test]
fn a_pass_that_backed_off_is_idle_whatever_it_did() {
    assert!(RunReport { done: 5, backed_off: true, ..RunReport::default() }.idle());
    assert!(RunReport { skipped: 3, ..RunReport::default() }.idle(), "only skipped");
    assert!(!RunReport { done: 1, skipped: 3, ..RunReport::default() }.idle());
    assert!(RunReport::default().idle());
}

#[test]
fn a_decoder_killed_by_a_signal_may_pass() {
    let path = Path::new("/music/a.flac");
    let killed = jewelcase_ffmpeg::Error::Killed { signal: 9, stderr: String::new() };
    assert!(matches!(Failure::from_ffmpeg(path, &killed), Failure::Attempt(_)));
    let rejected = jewelcase_ffmpeg::Error::Failed { status: Some(1), stderr: String::new() };
    assert!(matches!(Failure::from_ffmpeg(path, &rejected), Failure::Content(_)));
}
