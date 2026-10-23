//! The sandbox, against `tests/sandbox.wat`: a plugin written by hand that spins, waits in a host
//! call, grows its memory or a table, or fills its resource table, depending on the event it is
//! sent.

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::testing::{self, Memory};
use super::{CPU_BUDGET, Event, Forgotten, Host, Outcome, RunError, ScanFinished};

mod compiled;
mod limits;
mod lyrics;

/// The ticks of compute most runs here may spend, about 100 ms: well short of the fixture's
/// 500 ms wait.
const BUDGET: u64 = 100;

/// Runs the fixture once with `budget` ticks to handle `event`, returning its outcome and how
/// long the run took, timed after the fixture compiled.
async fn run(budget: u64, event: Event) -> (Outcome, Duration) {
    let file = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/host/tests/sandbox.wat"));
    let host = Host { budget, ..Host::new(None).expect("host") };
    let grants = testing::grants(&[]);
    host.compiled(&grants.plugin, file).await.expect("fixture compiles");
    let started = Instant::now();
    let outcome = host.run(file, grants, Memory::default(), event).await;
    (outcome, started.elapsed())
}

#[tokio::test]
async fn a_run_that_waits_past_its_budget_in_a_host_call_still_finishes() {
    let (outcome, took) = run(BUDGET, Event::Scheduled).await;
    assert_eq!(outcome.result, Ok("done".to_owned()));
    assert!(took >= Duration::from_millis(500), "it waited {took:?}");
    // The 500 ms wait costs one tick and the work after it a few more, well under half the budget.
    assert!((1..BUDGET / 2).contains(&outcome.ticks), "{} ticks", outcome.ticks);
}

/// On a thread and runtime of its own, so a spin the host never stops fails the test rather than
/// hanging it.
#[test]
fn a_spin_traps_at_its_compute_budget() {
    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
        sent.send(runtime.expect("runtime").block_on(run(BUDGET, Event::Run))).expect("sent");
    });
    let stopped = received.recv_timeout(Duration::from_secs(10));
    let (outcome, took) = stopped.expect("the spin was never stopped");
    assert_eq!(outcome.result, Err(RunError::OutOfCompute));
    assert_eq!(outcome.summary(), "the plugin ran past its compute budget");
    assert_eq!(outcome.ticks, BUDGET, "the whole budget is recorded as spent");
    // A tick lasts at least 1 ms, so the budget cannot run out sooner than that allows.
    assert!(took >= Duration::from_millis(BUDGET * 9 / 10), "it stopped after {took:?}");
    assert!(took < Duration::from_secs(1), "it stopped after {took:?}");
}

/// The clock is paused, so tokio skips ahead through the fixture's 10-minute wait, and the
/// time limit, not the wait, ends the run.
#[tokio::test(start_paused = true)]
async fn a_run_that_waits_past_the_time_limit_is_stopped() {
    let (outcome, _) =
        run(BUDGET, Event::SearchesForgotten(Forgotten { user: 7, searches: None })).await;
    assert_eq!(outcome.result, Err(RunError::OutOfTime));
    assert_eq!(outcome.summary(), "the plugin ran past its 300 s time limit");
}

/// Whether `outcome` is a crash whose message contains `why`.
fn crashed(outcome: &Outcome, why: &str) -> bool {
    matches!(&outcome.result, Err(RunError::Crashed(message)) if message.contains(why))
}

#[tokio::test]
async fn growing_memory_past_the_limit_is_refused() {
    let (outcome, _) = run(BUDGET, Event::CheckSettings).await;
    assert!(crashed(&outcome, "unreachable"), "{}", outcome.summary());
    assert!(outcome.summary().starts_with("the plugin crashed: "), "{}", outcome.summary());
}

#[tokio::test]
async fn growing_a_table_past_the_limit_is_refused() {
    let counts =
        ScanFinished { files_seen: 0, added: 0, updated: 0, moved: 0, missing: 0, problems: 0 };
    let (outcome, _) = run(BUDGET, Event::ScanFinished(counts)).await;
    assert!(crashed(&outcome, "unreachable"), "{}", outcome.summary());
}

/// With the whole budget, so that only the cap, not the time its thousands of host calls take,
/// can stop the run.
#[tokio::test]
async fn filling_the_resource_table_is_refused() {
    let (outcome, _) = run(CPU_BUDGET, Event::Played(Vec::new())).await;
    assert!(crashed(&outcome, "resource table has no free keys"), "{}", outcome.summary());
}
