use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread::sleep;

use super::*;
use crate::{Config, Error, Ffmpeg, ProbeInfo};

/// An `Ffmpeg` whose ffmpeg and ffprobe are `tests/lingering.sh`.
fn lingering() -> Ffmpeg {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lingering.sh");
    Ffmpeg::new(Config { ffmpeg: script.clone(), ffprobe: script })
}

/// The pids `registry` holds.
fn registered(registry: &Registry) -> Vec<Pid> {
    registry.state.lock().unwrap().children.keys().copied().collect()
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

/// The state letter in `/proc/<pid>/stat`, such as `S` (sleeping) or `T` (stopped).
fn state(pid: Pid) -> char {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid.as_raw_pid())).unwrap();
    stat.rsplit_once(") ").unwrap().1.chars().next().unwrap()
}

/// Runs `run` against `tests/lingering.sh` with the input `go`, then lets the script close its
/// output by creating `go`, and asserts its child left the registry while it was still running,
/// before it was reaped.
fn unregisters_before_reaping(run: impl FnOnce(Ffmpeg, PathBuf) + Send + 'static) {
    let dir = tempfile::tempdir().unwrap();
    let go = dir.path().join("go");
    let ffmpeg = lingering();
    let registry = ffmpeg.registry().clone();
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
fn dropping_a_decode_kills_and_reaps_its_child() {
    let ffmpeg = lingering();
    let info =
        ProbeInfo { sample_rate: 8000, channels: 1, channel_layout: None, duration_secs: None };
    // The input never exists, so the script would run for about 7 seconds.
    let stream = ffmpeg.decode_pcm(Path::new("/nonexistent/input"), &info).unwrap();
    let pid = registered(ffmpeg.registry())[0];
    let dropping = Instant::now();
    drop(stream);
    assert!(dropping.elapsed() < Duration::from_secs(2), "not killed");
    assert!(registered(ffmpeg.registry()).is_empty());
    assert!(!Path::new(&format!("/proc/{}", pid.as_raw_pid())).exists(), "not reaped");
}

/// Makes the registered child `pid` look stalled, as if it had made no progress for longer than
/// [`STALL_TIMEOUT`]. Returns the flag the watchdog sets when it kills it.
fn stall(registry: &Registry, pid: Pid) -> Arc<AtomicBool> {
    let mut state = registry.state.lock().unwrap();
    let entry = state.children.get_mut(&pid).unwrap();
    entry.last_progress -= STALL_TIMEOUT + Duration::from_secs(1);
    entry.stalled.clone()
}

#[test]
fn a_decode_the_watchdog_kills_fails_as_stalled() {
    let ffmpeg = lingering();
    let info =
        ProbeInfo { sample_rate: 8000, channels: 1, channel_layout: None, duration_secs: None };
    let mut stream = ffmpeg.decode_pcm(Path::new("/nonexistent/input"), &info).unwrap();
    let registry = ffmpeg.registry();
    stall(registry, registered(registry)[0]);
    registry.reap_stalled();
    let read = stream.read_frames(&mut Vec::new(), 1);
    assert!(matches!(read, Err(Error::Stalled)), "{read:?}");
}

/// A `sleep` child registered with a new registry. It ends by itself after 10 seconds, so a
/// failed test leaves it behind for no longer.
fn registered_sleep() -> (Arc<Registry>, Child, Pid) {
    let registry = Registry::start();
    let child = Command::new("sleep").arg("10").spawn().unwrap();
    let pid = Pid::from_child(&child);
    registry.register(pid);
    (registry, child, pid)
}

#[test]
fn pausing_stops_children_and_resuming_continues_them() {
    let (registry, mut child, pid) = registered_sleep();
    registry.pause_all();
    wait_for("the child to stop", || state(pid) == 'T');
    registry.resume_all();
    wait_for("the child to continue", || state(pid) == 'S');
    registry.unregister(pid);
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn the_watchdog_kills_a_stalled_child() {
    let (registry, mut child, pid) = registered_sleep();
    let stalled = stall(&registry, pid);
    registry.reap_stalled();
    registry.unregister(pid);
    assert_eq!(child.wait().unwrap().signal(), Some(9));
    assert!(stalled.load(Ordering::SeqCst));
}
