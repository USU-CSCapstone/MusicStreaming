use std::cell::Cell;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;

use rustix::process::getpid;
use rustix::time::{ClockId, clock_gettime};

use super::*;
use crate::pooled::Pooled;
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

/// A `sample` of a child that is blocked and has used no CPU time.
fn blocked(_: Pid) -> io::Result<Sample> {
    Ok(Sample { running: false, cpu: Duration::ZERO })
}

#[test]
fn a_decode_the_watchdog_kills_fails_as_stalled() {
    let ffmpeg = lingering();
    let info =
        ProbeInfo { sample_rate: 8000, channels: 1, channel_layout: None, duration_secs: None };
    let mut stream = ffmpeg.decode_pcm(Path::new("/nonexistent/input"), &info).unwrap();
    ffmpeg.registry.reap_stalled(STALL_TIMEOUT + CHECK_EVERY, blocked);
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

/// A child blocked for good, which uses no CPU time, is killed once that much time has passed.
#[test]
fn the_watchdog_kills_a_child_blocked_without_progress() {
    let (registry, child, pid, stalled) = registered_sleep();
    wait_for("the child to sleep", || state(pid) == 'S');
    assert!(!sample(pid).unwrap().running);
    // Takes in whatever CPU time the child used to start.
    registry.reap_stalled(CHECK_EVERY, sample);
    assert!(!stalled.load(Ordering::SeqCst));
    registry.reap_stalled(STALL_TIMEOUT, sample);
    killed(&registry, child, pid, &stalled);
}

/// A child that spins, or waits for a CPU to spin on, samples as running.
#[test]
fn samples_a_busy_child_as_running() {
    // Counts for a few seconds, so a failed test leaves it behind for no longer.
    let counting = "i=0; while [ $i -lt 3000000 ]; do i=$((i + 1)); done";
    let mut child = Command::new("sh").args(["-c", counting]).spawn().unwrap();
    let pid = Pid::from_child(&child);
    wait_for("the child to sample as running", || sample(pid).unwrap().running);
    child.kill().unwrap();
    child.wait().unwrap();
}

/// A child that cannot be sampled counts as blocked.
#[test]
fn the_watchdog_kills_a_child_it_cannot_sample_once_that_much_time_has_passed() {
    let (registry, child, pid, stalled) = registered_sleep();
    registry.reap_stalled(STALL_TIMEOUT + CHECK_EVERY, |_| Err(io::Error::other("no /proc")));
    killed(&registry, child, pid, &stalled);
}

/// A child that gets only a sliver of the CPU because busier work has it is not stalled,
/// however much time passes: neither while its main thread waits for a CPU and gets none, nor
/// while its main thread waits on its other threads and they get a little.
#[test]
fn the_watchdog_spares_a_child_starved_of_the_cpu() {
    let (registry, mut child, pid, stalled) = registered_sleep();
    let (cpu, check) = (Cell::new(Duration::ZERO), Cell::new(0));
    let starved = |_| {
        check.set(check.get() + 1);
        let running = check.get() % 2 == 0;
        if !running {
            cpu.set(cpu.get() + Duration::from_millis(10));
        }
        Ok(Sample { running, cpu: cpu.get() })
    };
    for _ in 0..100 {
        registry.reap_stalled(CHECK_EVERY, starved);
    }
    registry.unregister(pid);
    assert!(!stalled.load(Ordering::SeqCst), "killed as stalled");
    child.kill().unwrap();
    child.wait().unwrap();
}

/// A child that spins on the CPU without progress is killed once it has run that long since its
/// last progress, though it spins on four threads and so uses four times as much CPU time.
#[test]
fn the_watchdog_kills_a_child_spinning_without_progress() {
    let (registry, child, pid, stalled) = registered_sleep();
    let cpu = Cell::new(Duration::ZERO);
    let spinning = |_| {
        cpu.set(cpu.get() + CHECK_EVERY * 4);
        Ok(Sample { running: true, cpu: cpu.get() })
    };
    for _ in 0..20 {
        registry.reap_stalled(CHECK_EVERY, spinning);
    }
    registry.progress(pid);
    for _ in 0..STALL_TIMEOUT.as_secs() {
        registry.reap_stalled(CHECK_EVERY, spinning);
    }
    assert!(!stalled.load(Ordering::SeqCst), "killed within the timeout of its progress");
    registry.reap_stalled(CHECK_EVERY, spinning);
    killed(&registry, child, pid, &stalled);
}

/// The CPU time sampled from `/proc` is the process's own, in seconds rather than clock ticks.
#[test]
fn samples_a_processs_cpu_time() {
    let process_cpu = || {
        let now = clock_gettime(ClockId::ProcessCPUTime);
        Duration::new(u64::try_from(now.tv_sec).unwrap(), u32::try_from(now.tv_nsec).unwrap())
    };
    let start = Instant::now();
    // This process spins until it has used enough CPU time for a wrong unit to show.
    while process_cpu() < Duration::from_millis(200) {
        assert!(start.elapsed() < Duration::from_secs(10), "this process got no CPU time");
    }
    let before = process_cpu();
    let sampled = sample(getpid()).unwrap().cpu;
    let after = process_cpu();
    // `/proc` rounds the user and system times down to a clock tick each.
    let tick = Duration::from_secs(1) / rustix::param::clock_ticks_per_second() as u32;
    assert!(before - tick * 2 <= sampled && sampled <= after, "{before:?} {sampled:?} {after:?}");
}

#[test]
fn parses_the_state_and_cpu_ticks_past_a_name_with_spaces_and_parentheses() {
    let stat = "42 (a) R (b) S 1 42 42 0 -1 4194304 100 0 0 0 7 5 3 2 20 0 1 0 123 4096 100\n";
    assert_eq!(parse_stat(stat), Some(("S", 12)));
    assert_eq!(parse_stat("42 (a) S 1 42"), None);
}

/// Stopping kills a decode in flight, which fails as stopped rather than as a broken file, and
/// kills each child that starts after it.
#[test]
fn stopping_kills_every_child_and_each_that_starts_after() {
    let ffmpeg = lingering();
    let info =
        ProbeInfo { sample_rate: 8000, channels: 1, channel_layout: None, duration_secs: None };
    // The input never exists, so the script would run for about 7 seconds.
    let input = Path::new("/nonexistent/input");
    let mut stream = ffmpeg.decode_pcm(input, &info).unwrap();
    let stopping = Instant::now();
    ffmpeg.stop_background();
    let read = stream.read_frames(&mut Vec::new(), 1);
    assert!(matches!(read, Err(Error::Stopped)), "{read:?}");
    let probed = ffmpeg.probe(input);
    assert!(matches!(probed, Err(Error::Stopped)), "{probed:?}");
    assert!(stopping.elapsed() < Duration::from_secs(2), "{:?}", stopping.elapsed());
}

/// Whether a process still runs whose command line is `args`: one that is gone, or that ended
/// and waits to be reaped, does not.
fn runs(args: &[&str]) -> bool {
    let cmdline: Vec<u8> = args.iter().flat_map(|arg| [arg.as_bytes(), b"\0"].concat()).collect();
    std::fs::read_dir("/proc").unwrap().flatten().any(|entry| {
        let pid = entry.path();
        let stat = std::fs::read_to_string(pid.join("stat")).unwrap_or_default();
        std::fs::read(pid.join("cmdline")).is_ok_and(|read| read == cmdline)
            && stat.rsplit_once(") ").is_some_and(|(_, rest)| !rest.starts_with('Z'))
    })
}

/// A child refused once it has spawned is killed and leaves the pool.
#[test]
fn a_child_whose_standard_error_is_not_piped_is_refused_and_killed() {
    let registry = Registry::start().unwrap();
    // A duration no other test sleeps for, so that the child is told apart by its arguments.
    let args = ["sleep", "30.0417"];
    let mut command = Command::new(args[0]);
    command.arg(args[1]).stdout(Stdio::piped());
    let Err(Error::Io(error)) = Pooled::spawn(&registry, &mut command) else { panic!("spawned") };
    assert_eq!(error.to_string(), "ffmpeg's standard error was not piped");
    assert!(!runs(&args), "the child still runs");
    assert!(registered(&registry).is_empty(), "the child is still in the pool");
}
