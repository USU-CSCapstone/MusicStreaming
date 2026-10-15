//! How often each destination is asked (`requirements/plugins.md` §7). Requests to a host wait
//! in queues, each kept for one scope of requests there:
//!
//! - **A plugin's own**, unless its manifest says otherwise. No other plugin slows it.
//! - **A user's**: a plugin's requests in runs for one user who connected it, for a service
//!   that limits each account, so one user's runs do not use up another's.
//! - **The host's**: every plugin's, where a plugin allowed to has set a limit for a service
//!   that limits the whole server. Every plugin not exempt from it waits here too.
//!
//! A request goes at the latest turn of the queues it waits in, and each then keeps its own
//! gap after it. A `429` or `503` pauses only the scope of the request it answered, and after a
//! redirect, that scope where the request started too: as long as `Retry-After` asks, or for a
//! random wait that doubles in reach with each one in a row.
//!
//! A request waits for its turn rather than failing, unless the turn is more than [`MAX_WAIT`]
//! away, in which case it is refused with how long is left.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// The longest a request waits for its turn before it is refused instead. A run has five
/// minutes in all.
pub const MAX_WAIT: Duration = Duration::from_secs(60);
/// The longest pause a service can ask for. One that asks for a day is believed for an hour,
/// then asked again.
const MAX_PAUSE: Duration = Duration::from_secs(60 * 60);
/// The most the first pause without a `Retry-After` can be. Each in a row doubles it.
const BASE_PAUSE: Duration = Duration::from_secs(1);

/// Whose requests to a host wait together.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Every plugin's that is not exempt from the host's limit.
    Host,
    Plugin(String),
    /// A plugin's, in runs for one user.
    User(String, i64),
}

#[derive(Default)]
pub struct Limits {
    queues: Mutex<HashMap<(String, Scope), Queue>>,
}

/// One scope's requests to one host. Kept only while it holds a request up or counts a pause
/// that has not long passed.
struct Queue {
    /// The earliest the next request may go.
    next: Instant,
    /// The `429`s and `503`s in a row, which each make the next pause reach twice as far.
    throttled: u32,
}

impl Limits {
    /// When a request to `host` may go, with its turn taken in each of `queues`, or how long
    /// until it may if that is more than [`MAX_WAIT`]. Each queue is a scope and the gap it
    /// keeps after a request. Hosts are lowercase, so each has one set of queues.
    pub fn turn(
        &self,
        host: &str,
        queues: &[(Scope, Duration)],
        now: Instant,
    ) -> Result<Instant, Duration> {
        let mut kept = self.queues.lock().expect("limits lock");
        let key = |scope: &Scope| (host.to_owned(), scope.clone());
        let turns = queues.iter().filter_map(|(scope, _)| kept.get(&key(scope)));
        let start = turns.map(|queue| queue.next).fold(now, Instant::max);
        if start > now + MAX_WAIT {
            return Err(start - now);
        }
        for (scope, gap) in queues {
            let queue = kept.entry(key(scope)).or_insert(Queue { next: now, throttled: 0 });
            // From when it goes, so no queue thinks it went earlier than it did.
            queue.next = start + *gap;
            // Most hosts have no limit and no pause, and are never kept.
            if queue.idle(now) {
                kept.remove(&key(scope));
            }
        }
        Ok(start)
    }

    /// What `host` answered a request in `scope` with. A `429` or `503` pauses that scope's
    /// requests there; anything else ends its run of them. `clock` is the wall-clock time, for
    /// a `Retry-After` given as a date. `host` is lowercase, as for [`Limits::turn`].
    pub fn answered(
        &self,
        host: &str,
        scope: &Scope,
        status: u16,
        retry_after: Option<&str>,
        now: Instant,
        clock: SystemTime,
    ) {
        let key = (host.to_owned(), scope.clone());
        let mut kept = self.queues.lock().expect("limits lock");
        if !matches!(status, 429 | 503) {
            if let Some(queue) = kept.get_mut(&key) {
                queue.throttled = 0;
                if queue.idle(now) {
                    kept.remove(&key);
                }
            }
            return;
        }
        // Throttles are rare, so forgetting every queue that is done with here costs little,
        // and a run of them whose pause long passed starts again from none.
        kept.retain(|_, queue| !queue.idle(now));
        let queue = kept.entry(key).or_insert(Queue { next: now, throttled: 0 });
        let asked = retry_after.and_then(|value| parse_retry_after(value, clock));
        let until = now + pause(asked, queue.throttled, fastrand::f64());
        queue.next = queue.next.max(until);
        queue.throttled = queue.throttled.saturating_add(1);
    }
}

impl Queue {
    /// Holding nothing up and counting no pause, so no different from not being kept. A run of
    /// pauses is over once the last has long passed, though no answer since ended it.
    fn idle(&self, now: Instant) -> bool {
        self.next <= now && (self.throttled == 0 || self.next + MAX_PAUSE <= now)
    }
}

/// How long a `429` or `503` pauses: as long as `asked` by `Retry-After`, up to [`MAX_PAUSE`].
/// Without one, "full jitter": a random wait up to a reach that doubles with each of the
/// `throttled` before it in a row, so plugins told together do not all come back together.
/// `fraction` is the random draw, from 0 up to 1.
fn pause(asked: Option<Duration>, throttled: u32, fraction: f64) -> Duration {
    let reach = BASE_PAUSE.saturating_mul(2_u32.saturating_pow(throttled));
    asked.unwrap_or_else(|| reach.min(MAX_PAUSE).mul_f64(fraction)).min(MAX_PAUSE)
}

/// `Retry-After` as a wait: whole seconds, or a date (RFC 9110 §10.2.3).
fn parse_retry_after(value: &str, clock: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(at.duration_since(clock).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);
    const HOST: &str = "musicbrainz.org";

    fn own(plugin: &str) -> Scope {
        Scope::Plugin(plugin.into())
    }

    fn user(id: i64) -> Scope {
        Scope::User("scrobbler".into(), id)
    }

    /// A request in `scope` alone, which keeps no gap.
    fn alone(scope: Scope) -> [(Scope, Duration); 1] {
        [(scope, Duration::ZERO)]
    }

    /// A request in `scope` that also waits in the host's queue, a second apart.
    fn with_host(scope: Scope) -> [(Scope, Duration); 2] {
        [(scope, Duration::ZERO), (Scope::Host, SECOND)]
    }

    /// The `429`s in a row counted for `scope`, if it is kept.
    fn throttled(limits: &Limits, scope: Scope) -> Option<u32> {
        let kept = limits.queues.lock().unwrap();
        kept.get(&(HOST.into(), scope)).map(|queue| queue.throttled)
    }

    #[test]
    fn a_host_nobody_limits_is_never_held_up_or_kept() {
        let (limits, now) = (Limits::default(), Instant::now());
        for _ in 0..3 {
            assert_eq!(limits.turn("lrclib.net", &alone(own("lyrics")), now), Ok(now));
        }
        limits.answered("lrclib.net", &own("lyrics"), 200, None, now, SystemTime::now());
        assert!(limits.queues.lock().unwrap().is_empty());
    }

    #[test]
    fn a_plugins_own_pace_holds_up_no_other_plugin() {
        let (limits, now) = (Limits::default(), Instant::now());
        let artwork = [(own("artwork"), SECOND)];
        assert_eq!(limits.turn(HOST, &artwork, now), Ok(now));
        assert_eq!(limits.turn(HOST, &artwork, now), Ok(now + SECOND));
        assert_eq!(limits.turn(HOST, &alone(own("biographies")), now), Ok(now));
    }

    #[test]
    fn each_user_is_paced_apart() {
        let (limits, now) = (Limits::default(), Instant::now());
        let paced = |id| [(user(id), SECOND)];
        assert_eq!(limits.turn(HOST, &paced(1), now), Ok(now));
        assert_eq!(limits.turn(HOST, &paced(2), now), Ok(now));
        assert_eq!(limits.turn(HOST, &paced(1), now), Ok(now + SECOND));
    }

    #[test]
    fn a_host_limit_paces_plugins_that_declared_nothing_there() {
        let (limits, now) = (Limits::default(), Instant::now());
        assert_eq!(limits.turn(HOST, &with_host(own("artwork")), now), Ok(now));
        assert_eq!(limits.turn(HOST, &with_host(own("biographies")), now), Ok(now + SECOND));
        // The host's gap is the one given, which the run's grants say is the strictest.
        let stricter = [(own("artwork"), Duration::ZERO), (Scope::Host, 5 * SECOND)];
        assert_eq!(limits.turn(HOST, &stricter, now), Ok(now + 2 * SECOND));
        assert_eq!(limits.turn(HOST, &with_host(own("artwork")), now), Ok(now + 7 * SECOND));
        // A plugin exempt from it does not wait in the host's queue.
        assert_eq!(limits.turn(HOST, &alone(own("exempt")), now), Ok(now));
    }

    #[test]
    fn a_request_goes_at_the_latest_of_its_queues_and_each_keeps_its_gap_from_then() {
        let (limits, now) = (Limits::default(), Instant::now());
        // Its own queue is 3 s out, the host's free.
        limits.turn(HOST, &[(own("artwork"), 3 * SECOND)], now).unwrap();
        let both = [(own("artwork"), 3 * SECOND), (Scope::Host, SECOND)];
        assert_eq!(limits.turn(HOST, &both, now), Ok(now + 3 * SECOND));
        // The host's next turn is a second after the request went, not after now.
        assert_eq!(limits.turn(HOST, &with_host(own("other")), now), Ok(now + 4 * SECOND));
        assert_eq!(limits.turn(HOST, &both, now), Ok(now + 6 * SECOND));
    }

    #[test]
    fn a_turn_too_far_away_is_refused_and_takes_nothing() {
        let (limits, now) = (Limits::default(), Instant::now());
        let slow = [(own("artwork"), 50 * SECOND), (Scope::Host, 70 * SECOND)];
        assert_eq!(limits.turn(HOST, &slow, now), Ok(now));
        assert_eq!(limits.turn(HOST, &slow, now), Err(70 * SECOND));
        // The refused request took no turn in either queue.
        assert_eq!(limits.turn(HOST, &[(own("artwork"), SECOND)], now), Ok(now + 50 * SECOND));
        let later = now + 20 * SECOND;
        assert_eq!(limits.turn(HOST, &with_host(own("other")), later), Ok(now + 70 * SECOND));
    }

    #[test]
    fn a_429_pauses_only_the_scope_it_answered() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        let paused = now + 5 * SECOND;
        limits.answered(HOST, &own("told"), 429, Some("5"), now, clock);
        assert_eq!(limits.turn(HOST, &alone(own("told")), now), Ok(paused));
        assert_eq!(limits.turn(HOST, &alone(own("other")), now), Ok(now));
        assert_eq!(limits.turn(HOST, &with_host(own("other")), now), Ok(now));

        limits.answered(HOST, &user(1), 429, Some("5"), now, clock);
        assert_eq!(limits.turn(HOST, &alone(user(1)), now), Ok(paused));
        assert_eq!(limits.turn(HOST, &alone(user(2)), now), Ok(now));
    }

    #[test]
    fn a_429_to_a_host_scoped_request_holds_up_every_plugin_the_limit_binds() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        limits.answered(HOST, &Scope::Host, 429, Some("5"), now, clock);
        // One that declared nothing there, but is bound by the host's limit.
        assert_eq!(limits.turn(HOST, &with_host(own("other")), now), Ok(now + 5 * SECOND));
        // One exempt from it.
        assert_eq!(limits.turn(HOST, &alone(own("exempt")), now), Ok(now));
    }

    #[test]
    fn retry_after_may_be_a_date() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        let at = httpdate::fmt_http_date(clock + 20 * SECOND);
        limits.answered(HOST, &own("p"), 503, Some(&at), now, clock);
        let turn = limits.turn(HOST, &alone(own("p")), now).unwrap();
        // A date is to the second.
        assert!((now + 19 * SECOND..=now + 20 * SECOND).contains(&turn));
    }

    #[test]
    fn without_retry_after_each_pause_in_a_row_reaches_twice_as_far_until_one_succeeds() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        for (status, reach) in [(429, 1), (503, 2), (429, 4)] {
            limits.answered(HOST, &own("p"), status, None, now, clock);
            let turn = limits.turn(HOST, &alone(own("p")), now).unwrap();
            assert!(turn <= now + reach * SECOND, "{reach}");
        }
        assert_eq!(throttled(&limits, own("p")), Some(3));
        // Another answer ends the run, and an idle queue is forgotten.
        limits.answered(HOST, &own("p"), 500, None, now + 5 * SECOND, clock);
        assert_eq!(throttled(&limits, own("p")), None);
    }

    #[test]
    fn a_run_of_pauses_is_forgotten_once_its_pause_has_long_passed() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        for _ in 0..5 {
            limits.answered(HOST, &own("p"), 429, Some("1"), now, clock);
        }
        assert_eq!(throttled(&limits, own("p")), Some(5));
        // No answer since ended the run, but its pause long passed: the next counts from none.
        let later = now + SECOND + MAX_PAUSE;
        limits.answered(HOST, &own("p"), 429, Some("1"), later, clock);
        assert_eq!(throttled(&limits, own("p")), Some(1));
    }

    #[test]
    fn a_throttle_forgets_every_queue_done_with() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        for host in ["a.example", "b.example", "c.example"] {
            limits.answered(host, &own("p"), 503, Some("1"), now, clock);
        }
        assert_eq!(limits.queues.lock().unwrap().len(), 3);
        let later = now + SECOND + MAX_PAUSE;
        limits.answered(HOST, &own("p"), 503, Some("1"), later, clock);
        let kept = limits.queues.lock().unwrap();
        assert_eq!(kept.keys().map(|(host, _)| host.as_str()).collect::<Vec<_>>(), [HOST]);
    }

    #[test]
    fn a_pause_without_retry_after_is_a_random_part_of_its_reach() {
        assert_eq!(pause(None, 0, 0.0), Duration::ZERO);
        assert_eq!(pause(None, 0, 0.5), SECOND / 2);
        assert_eq!(pause(None, 3, 0.5), 4 * SECOND);
        // The reach stops growing at the longest pause.
        assert_eq!(pause(None, 40, 0.5), MAX_PAUSE / 2);
        // `Retry-After` is believed as it is, up to the longest pause.
        assert_eq!(pause(Some(7 * SECOND), 3, 0.0), 7 * SECOND);
        assert_eq!(pause(Some(MAX_PAUSE * 24), 0, 0.0), MAX_PAUSE);
    }

    #[test]
    fn a_long_pause_is_refused_rather_than_waited_for() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        limits.answered(HOST, &own("p"), 429, Some("86400"), now, clock);
        assert_eq!(limits.turn(HOST, &alone(own("p")), now), Err(MAX_PAUSE));
        let later = now + MAX_PAUSE;
        assert_eq!(limits.turn(HOST, &alone(own("p")), later), Ok(later));
    }
}
