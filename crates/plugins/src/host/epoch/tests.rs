//! The epoch advances only while a run is live, never misses a run that starts as another ends,
//! and stops for good when dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use super::Epoch;

/// An epoch whose ticks are counted in the answer's counter.
fn counted() -> (Epoch, Arc<AtomicU64>) {
    let ticks = Arc::new(AtomicU64::default());
    let counter = ticks.clone();
    let epoch = Epoch::start(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    });
    (epoch.unwrap(), ticks)
}

/// Waits up to a second for `done`, and panics if it never is.
fn eventually(what: &str, done: impl Fn() -> bool) {
    for _ in 0..1000 {
        if done() {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("{what} never happened");
}

#[test]
fn it_stands_still_while_no_run_is_live() {
    let (_epoch, ticks) = counted();
    thread::sleep(Duration::from_millis(50));
    assert_eq!(ticks.load(Ordering::Relaxed), 0);
}

#[test]
fn it_advances_while_a_run_is_live_and_stops_once_the_last_ends() {
    let (epoch, ticks) = counted();
    let (first, second) = (epoch.live(), epoch.live());
    eventually("ticking", || ticks.load(Ordering::Relaxed) >= 5);
    drop(first);
    let before = ticks.load(Ordering::Relaxed);
    eventually("ticking for the run still live", || ticks.load(Ordering::Relaxed) > before);
    drop(second);
    let ended = ticks.load(Ordering::Relaxed);
    thread::sleep(Duration::from_millis(50));
    // The thread may have checked for a run just before the last ended, and tick once more.
    assert!(ticks.load(Ordering::Relaxed) <= ended + 1, "it went on ticking");
}

/// Each run starts just as the one before it ends, while the thread may be going to sleep.
#[test]
fn a_run_that_starts_as_the_last_ends_still_advances_it() {
    let (epoch, ticks) = counted();
    for _ in 0..200 {
        drop(epoch.live());
        let _live = epoch.live();
        let before = ticks.load(Ordering::Relaxed);
        eventually("a tick for the new run", || ticks.load(Ordering::Relaxed) > before);
    }
}

/// The thread owns the counter's other handle, and drops it as it ends. A run comes and goes
/// first, so the thread is asleep when the epoch is dropped, and must be woken to end.
#[test]
fn dropping_it_ends_its_thread() {
    let (epoch, ticks) = counted();
    let live = epoch.live();
    eventually("a tick", || ticks.load(Ordering::Relaxed) > 0);
    drop(live);
    thread::sleep(Duration::from_millis(20));
    drop(epoch);
    eventually("the thread ending", || Arc::strong_count(&ticks) == 1);
}
