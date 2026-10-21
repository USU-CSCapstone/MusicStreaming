//! Work a suspended scan, or a request the store could not record, left undone, held until a
//! wait has passed and then queued again (`design/scanning.md` §9).

use std::time::{Duration, Instant};

use super::coalesce_scopes;
use crate::scan::Undone;
use crate::scan::{Scan, ScanState, Trigger};
use crate::scope::Scope;

/// The wait before work first left undone is queued again.
const FIRST_WAIT: Duration = Duration::from_secs(60);

/// The longest wait before work left undone is queued again.
const MAX_WAIT: Duration = Duration::from_secs(30 * 60);

/// How many retries in a row of work the store failed on fail on the store too before that work
/// is dropped: six, after waits of 1, 2, 4, 8, 16 and 30 minutes, about an hour.
const MAX_FAILED_RETRIES: u32 = 6;

/// One library's work waiting to be queued again, how long the next wait is, and how often in a
/// row the work last retried has failed on the store again.
pub(super) struct Retry {
    waiting: Option<Waiting>,
    /// The scopes the last retry queued, until a scan completes them.
    retried: Vec<Scope>,
    /// The wait for work left undone while nothing waits. It doubles with each wait, up to
    /// [`MAX_WAIT`], until a scan completes what the last retry queued.
    wait: Duration,
    /// How many retries in a row of the work in `retried` the store has failed on again: scans
    /// covering it that ended with work the store failed on, since one that ended without.
    failures: u32,
}

/// Work waiting to be queued again.
struct Waiting {
    trigger: Trigger,
    scopes: Vec<Scope>,
    due: Instant,
}

impl Retry {
    /// Nothing waiting, and the first wait [`FIRST_WAIT`].
    pub fn new() -> Retry {
        Retry { waiting: None, retried: Vec::new(), wait: FIRST_WAIT, failures: 0 }
    }

    /// Holds `scopes`, left undone by a scan or request with `trigger`, until a wait has passed.
    /// Work already waiting takes them in and keeps its time, so retries never compound;
    /// otherwise they wait from `now` for the current wait, and the next wait doubles.
    pub fn add(&mut self, trigger: Trigger, scopes: Vec<Scope>, now: Instant) {
        if scopes.is_empty() {
            return;
        }
        if let Some(waiting) = &mut self.waiting {
            waiting.scopes.extend(scopes);
            waiting.scopes = coalesce_scopes(std::mem::take(&mut waiting.scopes));
            return;
        }
        let wait = self.wait;
        tracing::info!(?trigger, ?wait, "work left undone is queued again after a wait");
        let scopes = coalesce_scopes(scopes);
        self.waiting = Some(Waiting { trigger, scopes, due: now + self.wait });
        self.wait = (self.wait * 2).min(MAX_WAIT);
    }

    /// Notes how `scan` ended, and holds what it left `undone` until a wait has passed ([`add`]).
    ///
    /// A scan covering what the last retry queued is that work's retry. One that ends with work
    /// the store failed on counts a failure in a row, and once [`MAX_FAILED_RETRIES`] have, the
    /// failed scopes the retry covered are dropped to wait for the next trigger, since a failure
    /// that recurs at the same file would otherwise walk the scope again every wait. One that
    /// ends without starts the count again, and one that completes makes the next wait
    /// [`FIRST_WAIT`] again. Work no retry covered, a root that did not answer, and what a
    /// request could not record always wait.
    ///
    /// [`add`]: Retry::add
    pub fn ended(&mut self, scan: &Scan, undone: Undone, now: Instant) {
        let Undone { mut unanswered, mut failed } = undone;
        if !self.retried.is_empty() && scan.covers(&self.retried) {
            if failed.is_empty() {
                self.failures = 0;
            } else {
                self.failures += 1;
            }
            if self.failures >= MAX_FAILED_RETRIES {
                let retried = &self.retried;
                failed.retain(|scope| !retried.iter().any(|r| r.covers(scope)));
                tracing::error!(?retried, "the store keeps failing; waiting for the next trigger");
                self.failures = 0;
            }
            if scan.state == ScanState::Completed {
                self.retried.clear();
                self.wait = FIRST_WAIT;
            }
        }
        unanswered.extend(failed);
        self.add(scan.trigger, unanswered, now);
    }

    /// Notes that `scan` is starting. One that covers the waiting work and walks all of it, not
    /// resuming from a cursor past some, does that work, so the work no longer waits, and counts
    /// as its retry.
    pub fn started(&mut self, scan: &Scan) {
        let does = |waiting: &mut Waiting| scan.cursor.is_none() && scan.covers(&waiting.scopes);
        if let Some(waiting) = self.waiting.take_if(does) {
            self.retried = waiting.scopes;
        }
    }

    /// When the waiting work is due, if any waits.
    pub fn due(&self) -> Option<Instant> {
        self.waiting.as_ref().map(|waiting| waiting.due)
    }

    /// The waiting work's trigger and scopes, once it is due at `now`, to queue. They count as
    /// retried until a scan completes them.
    pub fn take_due(&mut self, now: Instant) -> Option<(Trigger, Vec<Scope>)> {
        let waiting = self.waiting.take_if(|waiting| waiting.due <= now)?;
        self.retried.clone_from(&waiting.scopes);
        Some((waiting.trigger, waiting.scopes))
    }

    /// Makes the waiting work due at `due`, returning whether any waits.
    #[cfg(feature = "testing")]
    pub fn hurry(&mut self, due: Instant) -> bool {
        let Some(waiting) = &mut self.waiting else {
            return false;
        };
        waiting.due = due;
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::scan::{Cursor, ScanProgress};
    use crate::scope::Depth;

    const MINUTE: Duration = Duration::from_secs(60);

    /// A scan of `scopes` that ended `state`.
    fn scan(state: ScanState, scopes: &[Scope]) -> Scan {
        Scan {
            id: 1,
            library: 1,
            trigger: Trigger::Watch,
            scopes: scopes.to_vec(),
            state,
            started_at: None,
            finished_at: None,
            progress: ScanProgress::default(),
            cursor: None,
        }
    }

    /// The folder `name` under the root `/m`, and all below it.
    fn folder(name: &str) -> Scope {
        Scope::folder(Path::new("/m"), Path::new("/m").join(name), Depth::Subtree)
    }

    /// `scopes` left undone because their root did not answer.
    fn unanswered(scopes: &[Scope]) -> Undone {
        Undone { unanswered: scopes.to_vec(), failed: Vec::new() }
    }

    /// `scopes` left undone because the store failed.
    fn failed(scopes: &[Scope]) -> Undone {
        Undone { unanswered: Vec::new(), failed: scopes.to_vec() }
    }

    /// Each wait starts once the work before it was retried and left undone again: a minute, then
    /// two, doubling up to thirty, and nothing is due before its wait has passed. A root that does
    /// not answer waits however often it is retried.
    #[test]
    fn the_wait_doubles_up_to_thirty_minutes() {
        let mut retry = Retry::new();
        let mut now = Instant::now();
        let mut waits = Vec::new();
        for _ in 0..20 {
            retry.ended(
                &scan(ScanState::Suspended, &[folder("a")]),
                unanswered(&[folder("a")]),
                now,
            );
            let due = retry.due().unwrap();
            assert_eq!(retry.take_due(due - Duration::from_millis(1)), None);
            assert_eq!(retry.take_due(due), Some((Trigger::Watch, vec![folder("a")])));
            assert_eq!(retry.due(), None);
            waits.push((due - now).as_secs() / 60);
            now = due + MINUTE;
        }
        assert_eq!(waits[..8], [1, 2, 4, 8, 16, 30, 30, 30]);
        assert_eq!(waits[19], 30);
    }

    /// Work the store failed on is retried six times, over waits of about an hour, and once the
    /// sixth retry fails too, what the retry covered is dropped to wait for the next trigger,
    /// while a root that did not answer, and work the retry did not cover, still wait.
    #[test]
    fn work_the_store_keeps_failing_on_is_dropped_after_six_retries() {
        let mut retry = Retry::new();
        let mut now = Instant::now();
        let mut waits = Vec::new();
        let a = [folder("a")];
        for _ in 0..6 {
            retry.ended(&scan(ScanState::Suspended, &a), failed(&a), now);
            let due = retry.due().unwrap();
            waits.push((due - now).as_secs() / 60);
            assert!(retry.take_due(due).is_some());
            now = due;
        }
        assert_eq!(waits, [1, 2, 4, 8, 16, 30]);
        let all = [folder("a"), folder("b"), folder("c")];
        let undone =
            Undone { unanswered: vec![folder("b")], failed: vec![folder("a"), folder("c")] };
        retry.ended(&scan(ScanState::Suspended, &all), undone, now);
        assert_eq!(
            retry.take_due(now + MAX_WAIT),
            Some((Trigger::Watch, vec![folder("b"), folder("c")]))
        );

        // The next retry that fails again starts a new count of six.
        retry.ended(&scan(ScanState::Suspended, &all), failed(&[folder("c")]), now);
        assert!(retry.due().is_some(), "the count did not start again after the drop");
    }

    /// Work the store failed on that no retry covered waits however much else has: seven
    /// requests the store could not record in a row all wait.
    #[test]
    fn fresh_work_the_store_failed_on_always_waits() {
        let mut retry = Retry::new();
        let now = Instant::now();
        let folders: Vec<Scope> = (1..=7).map(|n| folder(&format!("f{n}"))).collect();
        for f in &folders {
            retry.add(Trigger::Watch, vec![f.clone()], now);
        }
        assert_eq!(retry.take_due(now + MINUTE), Some((Trigger::Watch, folders)));
    }

    /// A retry that ends without a store failure starts the count again, whether it completes or
    /// is suspended because its root did not answer, so later failures each get their own six.
    #[test]
    fn a_retry_without_a_store_failure_starts_the_count_again() {
        let mut retry = Retry::new();
        let now = Instant::now();
        let (a, b) = ([folder("a")], [folder("b")]);
        let fail = |retry: &mut Retry, times| {
            for _ in 0..times {
                retry.ended(&scan(ScanState::Suspended, &a), failed(&a), now);
                assert!(retry.take_due(now + MAX_WAIT).is_some(), "the retry was dropped");
            }
        };
        fail(&mut retry, 6);
        retry.ended(&scan(ScanState::Suspended, &a), unanswered(&a), now);
        assert!(retry.take_due(now + MAX_WAIT).is_some());
        fail(&mut retry, 5);

        retry.ended(&scan(ScanState::Completed, &a), Undone::default(), now);
        retry.ended(&scan(ScanState::Suspended, &b), failed(&b), now);
        assert_eq!(retry.take_due(now + MAX_WAIT), Some((Trigger::Watch, b.to_vec())));
    }

    /// Work left undone while other work waits joins it and keeps its time, so one retry queues
    /// it all, each scope once, and the next wait doubles only once.
    #[test]
    fn work_left_while_a_retry_waits_joins_it() {
        let mut retry = Retry::new();
        let now = Instant::now();
        retry.add(Trigger::Initial, vec![folder("b"), folder("a")], now);
        let more =
            Undone { unanswered: vec![folder("a"), folder("b/c")], failed: vec![folder("d")] };
        retry.ended(&scan(ScanState::Suspended, &[]), more, now + MINUTE);
        assert_eq!(retry.due(), Some(now + MINUTE));
        let queued = retry.take_due(now + MINUTE);
        assert_eq!(queued, Some((Trigger::Initial, vec![folder("a"), folder("b"), folder("d")])));
        retry.add(Trigger::Watch, vec![folder("a")], now + MINUTE);
        assert_eq!(retry.due(), Some(now + 3 * MINUTE));
    }

    /// A scan that starts covering the waiting work does it, so the work no longer waits; one
    /// that does not cover all of it, or resumes from a cursor past some of it, leaves it waiting.
    #[test]
    fn a_scan_covering_the_waiting_work_takes_it_over() {
        let mut retry = Retry::new();
        let now = Instant::now();
        retry.add(Trigger::Watch, vec![folder("a/b"), folder("c")], now);
        retry.started(&scan(ScanState::Running, &[folder("a")]));
        assert_eq!(retry.due(), Some(now + MINUTE));
        let cursor = Some(Cursor { scope_index: 0, after_directory: Some("/m/z".into()) });
        retry.started(&Scan { cursor, ..scan(ScanState::Running, &[Scope::root("/m")]) });
        assert_eq!(retry.due(), Some(now + MINUTE));
        retry.started(&scan(ScanState::Running, &[folder("a"), folder("c")]));
        assert_eq!(retry.due(), None);
    }

    /// The wait goes back to a minute only once a scan completes what the last retry queued: not
    /// for a scan that completes other work, nor for one that left the retried work undone, nor
    /// for one that completes before any retry was queued.
    #[test]
    fn the_wait_resets_once_the_retried_work_completes() {
        let mut retry = Retry::new();
        let now = Instant::now();
        let a = vec![folder("a")];
        retry.add(Trigger::Watch, a.clone(), now);
        retry.ended(&scan(ScanState::Completed, &[folder("b")]), Undone::default(), now);
        assert!(retry.take_due(now + 30 * MINUTE).is_some());
        retry.add(Trigger::Watch, a.clone(), now);
        assert_eq!(retry.due(), Some(now + 2 * MINUTE));
        assert!(retry.take_due(now + 30 * MINUTE).is_some());

        retry.ended(&scan(ScanState::Completed, &[folder("b")]), Undone::default(), now);
        retry.ended(&scan(ScanState::Suspended, &a), Undone::default(), now);
        retry.add(Trigger::Watch, a.clone(), now);
        assert_eq!(retry.due(), Some(now + 4 * MINUTE));
        assert!(retry.take_due(now + 30 * MINUTE).is_some());

        retry.ended(&scan(ScanState::Completed, &[Scope::root("/m")]), Undone::default(), now);
        retry.add(Trigger::Watch, a, now);
        assert_eq!(retry.due(), Some(now + MINUTE));
    }
}
