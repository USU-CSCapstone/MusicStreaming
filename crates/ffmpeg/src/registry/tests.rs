use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread::sleep;

use super::*;
use crate::{Config, Error, Ffmpeg, ProbeInfo};

/// An `Ffmpeg` whose ffmpeg and ffprobe are `tests/lingering.sh`.
fn lingering() -> Ffmpeg {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lingering.sh");
    Ffmpeg::new(Config { ffmpeg: script.clone(), ffprobe: script }).unwrap()
}

/// The pids `registry` holds.
fn registered(registry: &Registry) -> Vec<Pid> {
    registry.children.lock().unwrap().keys().copied().collect()
}

/// Waits up to 5 seconds for `done` to hold, and panics if it doesn't.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

/// Runs `run` against `tests/lingering.sh` with the input `go`, then lets the script close its
/// output by creating `go`, and asserts its child left the registry while it was still running,
/// before it was reaped.
fn unregisters_before_reaping(run: impl FnOnce(Ffmpeg, PathBuf) + Send + 'static) {
    let dir = tempfile::tempdir().unwrap();
    let go = dir.path().join("go");
    let ffmpeg = lingering();
    let registry = ffmpeg.registry.clone();
    let running = std::thread::spawn({
        let go = go.clone();
        move || run(ffmpeg, go)
    });
    wait_for("the child to register", || registered(&registry).len() == 1);
    let pid = registered(&registry)[0];
    std::fs::write(&go, "").unwrap();
    wait_for("the child to unregister", || registered(&registry).is_empty());
    assert!(
        Path::new(&format!("/proc/{}", pid.as_raw_pid())).exists(),
        "the child was reaped before it was unregistered"
    );
    running.join().unwrap();
}

#[test]
fn a_probe_unregisters_its_child_before_reaping_it() {
    unregisters_before_reaping(|ffmpeg, input| {
        let error = ffmpeg.probe(&input).unwrap_err();
        assert!(matches!(error, Error::Probe(_)), "{error:?}");
    });
}

#[test]
fn a_decode_unregisters_its_child_before_reaping_it() {
    unregisters_before_reaping(|ffmpeg, input| {
        let info =
            ProbeInfo { sample_rate: 8000, channels: 1, channel_layout: None, duration_secs: None };
        let mut stream = ffmpeg.decode_pcm(&input, &info).unwrap();
        assert_eq!(stream.read_frames(&mut Vec::new(), 1).unwrap(), 0);
        assert_eq!(stream.read_frames(&mut Vec::new(), 1).unwrap(), 0, "a read after the end");
    });
}

#[test]
fn an_image_decode_unregisters_its_child_before_reaping_it() {
    unregisters_before_reaping(|ffmpeg, input| {
        let error = ffmpeg.decode_image(&input, 8).unwrap_err();
        assert!(matches!(error, Error::Image(_)), "{error:?}");
    });
}

#[test]
fn dropping_a_decode_kills_and_reaps_its_child() {
    let ffmpeg = lingering();
    let info =
        ProbeInfo { sample_rate: 8000, channels: 1, channel_layout: None, duration_secs: None };
    // The input never exists, so the script would run for about 7 seconds.
    let stream = ffmpeg.decode_pcm(Path::new("/nonexistent/input"), &info).unwrap();
    let pid = registered(&ffmpeg.registry)[0];
    let dropping = Instant::now();
    drop(stream);
    assert!(dropping.elapsed() < Duration::from_secs(2), "not killed");
    assert!(registered(&ffmpeg.registry).is_empty());
    assert!(!Path::new(&format!("/proc/{}", pid.as_raw_pid())).exists(), "not reaped");
}

#[test]
fn a_decode_the_watchdog_kills_fails_as_stalled() {
    let ffmpeg = lingering();
    let info =
        ProbeInfo { sample_rate: 8000, channels: 1, channel_layout: None, duration_secs: None };
    let mut stream = ffmpeg.decode_pcm(Path::new("/nonexistent/input"), &info).unwrap();
    ffmpeg.registry.reap_stalled(Instant::now() + STALL_TIMEOUT + CHECK_EVERY);
    let read = stream.read_frames(&mut Vec::new(), 1);
    assert!(matches!(read, Err(Error::Stalled)), "{read:?}");
}

/// A `sleep` child registered with a new registry that has no watchdog thread, and the flag
/// the watchdog sets if it kills it. It ends by itself after 10 seconds, so a failed test
/// leaves it behind for no longer.
fn registered_sleep() -> (Registry, Child, Pid, Arc<AtomicBool>) {
    let registry = Registry::default();
    let child = Command::new("sleep").arg("10").spawn().unwrap();
    let pid = Pid::from_child(&child);
    let stalled = registry.register(pid);
    (registry, child, pid, stalled)
}

/// Unregisters the child `pid` and asserts it was killed as stalled.
fn killed(registry: &Registry, mut child: Child, pid: Pid, stalled: &AtomicBool) {
    registry.unregister(pid);
    assert_eq!(child.wait().unwrap().signal(), Some(9));
    assert!(stalled.load(Ordering::SeqCst));
}

/// A child with no progress is killed once [`STALL_TIMEOUT`] has passed since it registered.
#[test]
fn the_watchdog_kills_a_child_without_progress() {
    // Before the registration, so the first check is no later than the timeout after it.
    let before = Instant::now();
    let (registry, child, pid, stalled) = registered_sleep();
    registry.reap_stalled(before + STALL_TIMEOUT);
    assert!(!stalled.load(Ordering::SeqCst), "killed within the timeout");
    registry.reap_stalled(Instant::now() + STALL_TIMEOUT + CHECK_EVERY);
    killed(&registry, child, pid, &stalled);
}

/// Progress restarts a child's clock, so it outlives the timeout counted from its registration.
#[test]
fn progress_keeps_a_child_alive() {
    let (registry, mut child, pid, stalled) = registered_sleep();
    let start = Instant::now();
    registry.progress(pid);
    // The progress is no earlier than `start`, so this check is within the timeout of it.
    registry.reap_stalled(start + STALL_TIMEOUT);
    registry.unregister(pid);
    assert!(!stalled.load(Ordering::SeqCst), "killed as stalled");
    child.kill().unwrap();
    child.wait().unwrap();
}
