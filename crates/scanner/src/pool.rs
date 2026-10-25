//! One pool of background threads for every library's analysis and placeholders
//! (`design/scanning.md` §10).
//!
//! Each library's analysis and its placeholders are a [`Job`] each. The pool's threads take the
//! jobs that are due in turn, so one library's backlog never starves another's, and a job with
//! work may run on several threads at once. Each thread runs one ffmpeg child at a time, so the
//! pool's threads bound background ffmpeg on the whole host. No library runs on more threads at
//! once than the pool's size, and a pool with jobs of more than one library has a thread beyond
//! it, so a library busy alone has the whole size, and one whose passes hang, as on a dead mount,
//! leaves a thread for the others. A pass that panics is logged, and its job waits as an idle one
//! does.

use std::collections::{HashMap, HashSet};
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::jobs::RunReport;
use crate::library::LibraryId;
use crate::worker::{Stop, Worker};
use jewelcase_core::Recover;

/// One library's background job: a pass over its queue, run again at once after a pass that did
/// something, and after a wait otherwise.
pub struct Job {
    library: LibraryId,
    /// How long the job waits after a pass that backed off or did nothing ([`RunReport::idle`]),
    /// or panicked.
    idle: Duration,
    pass: Box<dyn Fn(&Stop) -> RunReport + Send + Sync>,
}

impl Job {
    /// A job of `library` that runs `pass`, waiting `idle` after a pass that backed off, did
    /// nothing or panicked. Passes may run on several threads at once, so `pass` must not take a
    /// file that a pass running beside it has in hand. `pass` is given the [`Stop`] of the thread
    /// running it, and must return soon after it is asked.
    pub fn new(
        library: LibraryId,
        idle: Duration,
        pass: impl Fn(&Stop) -> RunReport + Send + Sync + 'static,
    ) -> Job {
        Job { library, idle, pass: Box::new(pass) }
    }
}

/// The threads that run every [`Job`], each at low priority, until the pool is dropped.
#[must_use = "dropping the pool stops its threads"]
pub struct Pool {
    workers: Vec<Worker>,
}

impl Pool {
    /// The pool's size on this host: half its cores, and at least one.
    pub fn size_for_host() -> usize {
        thread::available_parallelism().map_or(1, |cores| cores.get() / 2).max(1)
    }

    /// Runs `jobs` on `size` threads, each library on at most that many at once, and on one
    /// thread more when the jobs are of more than one library (`Schedule::new` says why). Fails
    /// if a thread cannot be started.
    pub fn start(jobs: Vec<Job>, size: usize) -> io::Result<Pool> {
        let schedule = Arc::new(Schedule::new(jobs, size));
        let start = |_| {
            let schedule = schedule.clone();
            Worker::spawn("scanner-decode", move |stop| run(&schedule, &stop))
        };
        let workers = (0..schedule.threads).map(start).collect::<io::Result<_>>()?;
        Ok(Pool { workers })
    }
}

impl Drop for Pool {
    /// Asks every thread to stop before waiting for any, so they finish their passes together.
    fn drop(&mut self) {
        self.workers.iter_mut().for_each(Worker::ask_to_stop);
    }
}

/// The jobs, and when each is next due.
struct Schedule {
    jobs: Vec<Job>,
    /// The most passes of one library's jobs that run at once.
    per_library: usize,
    /// How many threads run the jobs.
    threads: usize,
    due: Mutex<Due>,
}

/// When each job, by its place in [`Schedule::jobs`], is next due, the place to look from for
/// the next one to run, and how many passes of each library's jobs are running.
struct Due {
    at: Vec<Instant>,
    next: usize,
    running: HashMap<LibraryId, usize>,
}

/// One pool thread: lowers its priority, then runs due jobs until it is asked to stop.
fn run(schedule: &Schedule, stop: &Stop) {
    jewelcase_ffmpeg::lower_thread_priority();
    while let Some(wait) = schedule.run_next(stop) {
        if stop.wait(wait) {
            break;
        }
    }
}

impl Schedule {
    /// A schedule of `jobs`, all due now, each library running on at most `size` threads at once.
    /// Jobs of more than one library get a thread beyond `size`, so a library whose passes hang
    /// leaves one for the others, at the cost of one more pass, and so ffmpeg child, while two
    /// libraries have work at once.
    fn new(jobs: Vec<Job>, size: usize) -> Schedule {
        let libraries = jobs.iter().map(|job| job.library).collect::<HashSet<_>>().len();
        let threads = if libraries > 1 { size + 1 } else { size };
        let due = Due { at: vec![Instant::now(); jobs.len()], next: 0, running: HashMap::new() };
        Schedule { jobs, per_library: size, threads, due: Mutex::new(due) }
    }

    /// Runs a pass, given `stop`, of the first job due whose library is running fewer than its
    /// share of passes, looking from the one after the job last taken, and returns a wait of
    /// zero; or, when no such job is due, returns how long until one is. A pass that panics is logged, and
    /// its job waits as after an idle pass. Returns `None` when there are no jobs: while a thread
    /// is free, some library always runs fewer than its share.
    fn run_next(&self, stop: &Stop) -> Option<Duration> {
        let job = {
            let mut due = self.due.lock().recover();
            let now = Instant::now();
            let count = self.jobs.len();
            let open = |due: &Due, job: usize| {
                due.running.get(&self.jobs[job].library).copied().unwrap_or(0) < self.per_library
            };
            let mut order = (0..count).map(|i| (due.next + i) % count);
            let Some(job) = order.find(|&j| due.at[j] <= now && open(&due, j)) else {
                let soonest = (0..count).filter(|&j| open(&due, j)).map(|j| due.at[j]).min();
                return soonest.map(|at| at.saturating_duration_since(now));
            };
            due.next = (job + 1) % count;
            *due.running.entry(self.jobs[job].library).or_default() += 1;
            job
        };
        let Job { library, idle, pass } = &self.jobs[job];
        // The panic itself is reported by the panic hook as it happens.
        let idle_pass = match panic::catch_unwind(AssertUnwindSafe(|| pass(stop))) {
            Ok(report) => report.idle(),
            Err(_) => {
                tracing::error!(
                    library,
                    "a background job panicked; it waits before its next pass"
                );
                true
            }
        };
        let mut due = self.due.lock().recover();
        *due.running.entry(*library).or_default() -= 1;
        if idle_pass {
            due.at[job] = Instant::now() + *idle;
        }
        Some(Duration::ZERO)
    }
}

#[cfg(test)]
mod tests;
