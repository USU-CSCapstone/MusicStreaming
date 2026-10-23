use std::sync::Condvar;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// Longer than any test waits.
const LONG: Duration = Duration::from_secs(60);

/// A pass that did something.
const BUSY: RunReport = RunReport { done: 1, failed: 0, skipped: 0, backed_off: false };

/// Waits up to 5 seconds for `done` to hold, and panics if it doesn't.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

/// A job of `library` that counts its passes in `passes`, reports `report` from each, and waits
/// `idle` after an idle one.
fn counted(
    library: LibraryId,
    passes: &Arc<AtomicUsize>,
    idle: Duration,
    report: RunReport,
) -> Job {
    let passes = passes.clone();
    Job::new(library, idle, move || {
        passes.fetch_add(1, Ordering::SeqCst);
        report.clone()
    })
}

/// A job runs its next pass at once after a pass that did something, and waits after one that
/// found nothing to do.
#[test]
fn a_job_waits_only_when_idle() {
    let passes = Arc::new(AtomicUsize::new(0));
    let counter = passes.clone();
    let job = Job::new(1, LONG, move || {
        if counter.fetch_add(1, Ordering::SeqCst) < 3 { BUSY } else { RunReport::default() }
    });
    let pool = Pool::start(vec![job], 1).unwrap();
    wait_for("four passes", || passes.load(Ordering::SeqCst) >= 4);
    thread::sleep(Duration::from_millis(200));
    assert_eq!(passes.load(Ordering::SeqCst), 4, "an idle pass was not followed by a wait");
    drop(pool);
}

/// Two jobs of one library, both with work, take turns on one thread.
#[test]
fn jobs_with_work_take_turns_on_one_thread() {
    let (first, second) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let jobs = vec![counted(1, &first, LONG, BUSY), counted(1, &second, LONG, BUSY)];
    let pool = Pool::start(jobs, 1).unwrap();
    wait_for("passes of the second job", || second.load(Ordering::SeqCst) >= 20);
    drop(pool);
    let (first, second) = (first.load(Ordering::SeqCst), second.load(Ordering::SeqCst));
    assert!(first.abs_diff(second) <= 1, "{first} passes and {second}");
}

/// Three libraries' jobs, all with work, take turns on a pool of two threads, so none starves.
#[test]
fn jobs_with_work_take_turns() {
    let passes: [_; 3] = std::array::from_fn(|_| Arc::new(AtomicUsize::new(0)));
    let jobs =
        (1..).zip(&passes).map(|(library, passes)| counted(library, passes, LONG, BUSY)).collect();
    let pool = Pool::start(jobs, 1).unwrap();
    let least = || passes.iter().map(|p| p.load(Ordering::SeqCst)).min().unwrap();
    wait_for("passes of every job", || least() >= 20);
    drop(pool);
}

/// A job waiting after a pass that backed off holds up no other job on the same thread.
#[test]
fn a_job_backing_off_holds_up_no_other() {
    let (backing_off, busy) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let backed_off = RunReport { backed_off: true, ..RunReport::default() };
    let jobs = vec![counted(1, &backing_off, LONG, backed_off), counted(1, &busy, LONG, BUSY)];
    let pool = Pool::start(jobs, 1).unwrap();
    wait_for("passes of the busy job", || busy.load(Ordering::SeqCst) >= 20);
    drop(pool);
    assert_eq!(backing_off.load(Ordering::SeqCst), 1);
}

/// The passes running at once, and the most there have been.
#[derive(Default)]
struct Running {
    /// How many run now, and the most that have.
    count: Mutex<(usize, usize)>,
    changed: Condvar,
}

impl Running {
    /// A pass: counts itself in, waits up to `wait` for `others` more passes to run beside it,
    /// then counts itself out.
    fn pass(&self, others: usize, wait: Duration) -> RunReport {
        let mut count = self.count.lock().unwrap();
        count.0 += 1;
        count.1 = count.1.max(count.0);
        self.changed.notify_all();
        let target = count.0 + others;
        let mut count = self.changed.wait_timeout_while(count, wait, |c| c.0 < target).unwrap().0;
        count.0 -= 1;
        BUSY
    }

    /// The most passes that have run at once.
    fn most(&self) -> usize {
        self.count.lock().unwrap().1
    }
}

/// One library's job, with work, runs on every thread of the pool at once.
#[test]
fn one_job_runs_on_every_thread() {
    let running = Arc::new(Running::default());
    let job = Job::new(1, LONG, {
        let running = running.clone();
        move || running.pass(2, Duration::from_millis(100))
    });
    let pool = Pool::start(vec![job], 3).unwrap();
    wait_for("three passes at once", || running.most() == 3);
    drop(pool);
}

/// A library busy while another is idle runs on as many threads as the pool's size: the thread
/// beyond it is the only one kept for the other library.
#[test]
fn a_library_busy_alone_runs_on_the_whole_size() {
    let running = Arc::new(Running::default());
    let busy = Job::new(1, LONG, {
        let running = running.clone();
        move || running.pass(1, Duration::from_millis(100))
    });
    let idle = Job::new(2, LONG, RunReport::default);
    let pool = Pool::start(vec![busy, idle], 2).unwrap();
    wait_for("two passes at once", || running.most() == 2);
    drop(pool);
}

/// However many jobs have work, no more passes run at once than the pool has threads, so no more
/// ffmpeg children either: its size, and one more for jobs of more than one library.
#[test]
fn no_more_passes_run_at_once_than_threads() {
    let running = Arc::new(Running::default());
    let passes = Arc::new(AtomicUsize::new(0));
    let job = |library| {
        let (running, passes) = (running.clone(), passes.clone());
        Job::new(library, LONG, move || {
            passes.fetch_add(1, Ordering::SeqCst);
            running.pass(3, Duration::from_millis(20))
        })
    };
    let pool = Pool::start((1..=4).map(job).collect(), 2).unwrap();
    wait_for("passes", || passes.load(Ordering::SeqCst) >= 40);
    drop(pool);
    assert_eq!(running.most(), 3);
}

/// Dropping the pool asks every thread to stop before waiting for any, so each finishes only the
/// pass in hand, and they finish together.
#[test]
fn dropping_the_pool_stops_its_threads_together() {
    let pass = Duration::from_millis(500);
    let running = Arc::new(Running::default());
    let job = Job::new(1, LONG, {
        let running = running.clone();
        move || running.pass(10, pass)
    });
    let pool = Pool::start(vec![job], 2).unwrap();
    wait_for("two passes at once", || running.most() == 2);
    let stopping = Instant::now();
    drop(pool);
    // Waited for one after the other, the second thread would start another pass.
    assert!(stopping.elapsed() < pass * 3 / 2, "the stop took {:?}", stopping.elapsed());
}

/// With every job idle, the pool's threads sleep until the soonest is due, and no longer.
#[test]
fn an_idle_pool_wakes_for_the_soonest_job() {
    let (soon, late) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let idle = RunReport::default();
    let soon_job = counted(1, &soon, Duration::from_millis(30), idle.clone());
    let pool = Pool::start(vec![soon_job, counted(2, &late, LONG, idle)], 1).unwrap();
    let started = Instant::now();
    wait_for("passes of the job due soonest", || soon.load(Ordering::SeqCst) >= 3);
    assert!(started.elapsed() < Duration::from_secs(1), "took {:?}", started.elapsed());
    drop(pool);
}

/// A job of library 2, a healthy one, that counts its passes in `passes`, each idle and followed
/// by a 10 ms wait, so it runs often while the pool has a thread for it.
fn healthy(passes: &Arc<AtomicUsize>) -> Job {
    counted(2, passes, Duration::from_millis(10), RunReport::default())
}

/// A pass that panics costs no thread, and its job waits as an idle one does, while every
/// library goes on.
#[test]
fn a_panicking_pass_costs_no_thread() {
    let (panicked, passes) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let started = Instant::now();
    let panicking = Job::new(1, Duration::from_millis(200), {
        let panicked = panicked.clone();
        move || {
            panicked.fetch_add(1, Ordering::SeqCst);
            panic!("a decoder bug");
        }
    });
    // A size of one runs one pass of each library at a time, so its panics can be counted.
    let pool = Pool::start(vec![panicking, healthy(&passes)], 1).unwrap();
    wait_for("passes of the healthy job", || passes.load(Ordering::SeqCst) >= 10);
    wait_for("a second panic", || panicked.load(Ordering::SeqCst) >= 2);
    let panics = panicked.load(Ordering::SeqCst);
    let elapsed = started.elapsed();
    drop(pool);
    // One at the start, and one more after each 200 ms wait at most.
    let most = 1 + usize::try_from(elapsed.as_millis() / 200).unwrap();
    assert!(panics <= most, "{panics} panics in {elapsed:?}");
}

/// Shut until opened, then open for good.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    /// A pass that did something, once the gate is open, or after 10 seconds at most.
    fn pass(&self) -> RunReport {
        let open = self.open.lock().unwrap();
        let wait = Duration::from_secs(10);
        drop(self.opened.wait_timeout_while(open, wait, |open| !*open).unwrap());
        BUSY
    }

    /// Opens the gate.
    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

/// A library whose passes hang, as on a dead mount, in both its jobs, takes as many threads as
/// the pool's size, and the other library goes on in the one beyond it.
#[test]
fn a_hung_library_leaves_a_thread_for_the_others() {
    let (gate, passes) = (Arc::new(Gate::default()), Arc::new(AtomicUsize::new(0)));
    let hung = || {
        let gate = gate.clone();
        Job::new(1, LONG, move || gate.pass())
    };
    let pool = Pool::start(vec![hung(), hung(), healthy(&passes)], 2).unwrap();
    wait_for("passes of the healthy job", || passes.load(Ordering::SeqCst) >= 20);
    gate.open();
    drop(pool);
}

/// While a library with work runs on as many threads as the pool's size, the spare thread waits
/// for another library's job to be due, rather than looking again at once.
#[test]
fn the_spare_thread_waits_while_a_library_has_its_share() {
    let gate = Arc::new(Gate::default());
    let idle = Job::new(2, LONG, RunReport::default);
    let busy = Job::new(1, LONG, {
        let gate = gate.clone();
        move || gate.pass()
    });
    let schedule = Schedule::new(vec![idle, busy], 1);
    assert_eq!(schedule.threads, 2);
    assert_eq!(schedule.run_next(), Some(Duration::ZERO), "the idle library's first pass");
    thread::scope(|scope| {
        let busy = scope.spawn(|| schedule.run_next());
        let running = || schedule.due.lock().unwrap().running.get(&1).copied();
        wait_for("the busy library's pass", || running() == Some(1));
        let wait = schedule.run_next().unwrap();
        gate.open();
        assert!(wait > LONG / 2, "waits {wait:?}");
        assert_eq!(busy.join().unwrap(), Some(Duration::ZERO));
    });
}
