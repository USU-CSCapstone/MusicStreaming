//! The sandbox, against `tests/sandbox.wat`: a plugin written by hand that spins, waits in a host
//! call, or grows its memory, depending on the event it is sent.

use std::path::Path;
use std::time::{Duration, Instant};

use super::testing::{self, Memory};
use super::{Event, Host, Outcome};
use crate::manifest::Api;

/// The ticks of compute each run here may spend, about 100 ms: well short of the fixture's
/// 500 ms wait.
const BUDGET: u64 = 100;

/// Runs the fixture once with `BUDGET` to handle `event`, returning its outcome and how long the
/// run took, timed after the fixture compiled.
async fn run(event: Event) -> (Outcome, Duration) {
    let file = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/tests/sandbox.wat"));
    let host = Host { budget: BUDGET, ..Host::new().expect("host") };
    host.compiled(file).await.expect("fixture compiles");
    let started = Instant::now();
    let grants = testing::grants(&[]);
    let outcome = host.run(file, Api::V0_3, grants, Memory::default(), event).await;
    (outcome, started.elapsed())
}

#[tokio::test]
async fn a_run_that_waits_past_its_budget_in_a_host_call_still_finishes() {
    let (outcome, took) = run(Event::Scheduled).await;
    assert!(outcome.ok, "{}", outcome.summary);
    assert_eq!(outcome.summary, "done");
    assert!(took >= Duration::from_millis(500), "it waited {took:?}");
}

#[tokio::test]
async fn a_spin_traps_at_its_compute_budget() {
    let (outcome, took) = run(Event::Run).await;
    assert!(!outcome.ok);
    assert!(outcome.summary.contains("ran past its compute budget"), "{}", outcome.summary);
    // A tick lasts at least 1 ms, so the budget cannot run out sooner than that allows.
    assert!(took >= Duration::from_millis(BUDGET * 9 / 10), "it stopped after {took:?}");
    assert!(took < Duration::from_secs(1), "it stopped after {took:?}");
}

/// The clock is paused, so tokio skips ahead through the fixture's 10-minute wait, and the
/// time limit, not the wait, ends the run.
#[tokio::test(start_paused = true)]
async fn a_run_that_waits_past_the_time_limit_is_stopped() {
    let (outcome, _) = run(Event::SearchesForgotten(None)).await;
    assert!(!outcome.ok);
    assert!(outcome.summary.contains("time limit"), "{}", outcome.summary);
}

#[tokio::test]
async fn growing_memory_past_the_limit_is_refused() {
    let (outcome, _) = run(Event::CheckSettings).await;
    assert!(!outcome.ok);
    assert!(outcome.summary.contains("unreachable"), "{}", outcome.summary);
}
