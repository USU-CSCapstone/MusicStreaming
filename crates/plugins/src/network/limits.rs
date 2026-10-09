//! How often each destination is asked (`requirements/plugins.md` §7). A service's limit is
//! usually per address, so two plugins that each keep to it would break it together. Both
//! rules here are kept for every plugin at once:
//!
//! - **The strictest declared pace.** A plugin's manifest can declare a rate limit for a
//!   destination, and every request to that host keeps to the strictest any plugin declared.
//! - **Pauses a service asks for.** A `429`, or a `503` with `Retry-After`, pauses that host for
//!   every plugin, not just the one that was told.
//!
//! A request waits for its turn rather than failing, unless the turn is more than [`MAX_WAIT`]
//! away, in which case it is refused with how long is left.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// The longest a request waits for its turn before it is refused instead. A run has five
/// minutes in all.
pub const MAX_WAIT: Duration = Duration::from_secs(60);
/// The pause after a `429` that does not say how long to wait.
const DEFAULT_PAUSE: Duration = Duration::from_secs(30);
/// The longest pause a service can ask for. One that asks for a day is believed for an hour,
/// then asked again.
const MAX_PAUSE: Duration = Duration::from_secs(60 * 60);

#[derive(Default)]
pub struct Limits {
    destinations: Mutex<HashMap<String, Destination>>,
}

#[derive(Default)]
struct Destination {
    /// The gap each plugin declared between requests here. The widest is kept to.
    declared: HashMap<String, Duration>,
    /// The earliest the next request may go.
    next: Option<Instant>,
}

impl Limits {
    /// When a request from `plugin` to `host` may go, with that turn taken, or why it may not
    /// go soon. `declared` is the gap its manifest asks for there, if any.
    pub fn turn(
        &self,
        host: &str,
        plugin: &str,
        declared: Option<Duration>,
        now: Instant,
    ) -> Result<Instant, String> {
        let host = host.to_ascii_lowercase();
        let mut destinations = self.destinations.lock().expect("limits lock");
        // Most hosts have no limit and no pause, and are never recorded.
        if declared.is_none() && !destinations.contains_key(&host) {
            return Ok(now);
        }
        let destination = destinations.entry(host.clone()).or_default();
        match declared {
            Some(gap) => destination.declared.insert(plugin.to_owned(), gap),
            None => destination.declared.remove(plugin),
        };
        let start = destination.next.map_or(now, |next| next.max(now));
        if start > now + MAX_WAIT {
            let wait = (start - now).as_secs_f64().ceil();
            return Err(format!("{host} cannot be asked again for {wait} s"));
        }
        let gap = destination.declared.values().max().copied().unwrap_or_default();
        if destination.declared.is_empty() && start == now {
            // Nothing declared and no pause left: forget it.
            destinations.remove(&host);
        } else {
            destination.next = Some(start + gap);
        }
        Ok(start)
    }

    /// What `host` answered with. A `429`, or a `503` that says when to come back, pauses it
    /// for every plugin. `clock` is the wall-clock time, for a `Retry-After` given as a date.
    pub fn answered(
        &self,
        host: &str,
        status: u16,
        retry_after: Option<&str>,
        now: Instant,
        clock: SystemTime,
    ) {
        let asked = retry_after.and_then(|value| parse_retry_after(value, clock));
        let pause = match (status, asked) {
            (429, asked) => asked.unwrap_or(DEFAULT_PAUSE),
            (503, Some(asked)) => asked,
            _ => return,
        };
        let until = now + pause.min(MAX_PAUSE);
        let mut destinations = self.destinations.lock().expect("limits lock");
        let destination = destinations.entry(host.to_ascii_lowercase()).or_default();
        destination.next = Some(destination.next.map_or(until, |next| next.max(until)));
    }
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

    #[test]
    fn a_host_nobody_limits_is_never_held_up_or_recorded() {
        let (limits, now) = (Limits::default(), Instant::now());
        for _ in 0..3 {
            assert_eq!(limits.turn("lrclib.net", "lyrics", None, now), Ok(now));
        }
        assert!(limits.destinations.lock().unwrap().is_empty());
    }

    #[test]
    fn requests_are_spaced_at_the_declared_pace_across_plugins() {
        let (limits, now) = (Limits::default(), Instant::now());
        let host = "musicbrainz.org";
        assert_eq!(limits.turn(host, "artwork", Some(SECOND), now), Ok(now));
        assert_eq!(limits.turn(host, "artwork", Some(SECOND), now), Ok(now + SECOND));
        // A plugin that declared nothing still waits its turn behind the one that did.
        assert_eq!(limits.turn(host, "biographies", None, now), Ok(now + 2 * SECOND));
        // Hosts are compared without case.
        assert_eq!(
            limits.turn("MusicBrainz.org", "artwork", Some(SECOND), now),
            Ok(now + 3 * SECOND)
        );
    }

    #[test]
    fn the_strictest_declared_pace_wins() {
        let (limits, now) = (Limits::default(), Instant::now());
        let host = "api.example";
        let lenient = SECOND / 10;
        assert_eq!(limits.turn(host, "lenient", Some(lenient), now), Ok(now));
        assert_eq!(limits.turn(host, "strict", Some(2 * SECOND), now), Ok(now + lenient));
        // Once the strict plugin has declared its pace, the lenient one keeps to it too.
        assert_eq!(
            limits.turn(host, "lenient", Some(lenient), now),
            Ok(now + lenient + 2 * SECOND)
        );
    }

    #[test]
    fn a_429_pauses_the_host_for_everyone() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        limits.answered("api.example", 429, Some("5"), now, clock);
        assert_eq!(limits.turn("api.example", "other", None, now), Ok(now + 5 * SECOND));
        // Without a Retry-After, a 429 still pauses it.
        limits.answered("quiet.example", 429, None, now, clock);
        assert_eq!(limits.turn("quiet.example", "other", None, now), Ok(now + DEFAULT_PAUSE));
    }

    #[test]
    fn retry_after_may_be_a_date() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        let at = httpdate::fmt_http_date(clock + 20 * SECOND);
        limits.answered("api.example", 503, Some(&at), now, clock);
        let turn = limits.turn("api.example", "p", None, now).unwrap();
        // A date is to the second.
        assert!((now + 19 * SECOND..=now + 20 * SECOND).contains(&turn));
    }

    #[test]
    fn other_answers_pause_nothing() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        limits.answered("api.example", 503, None, now, clock);
        limits.answered("api.example", 500, Some("30"), now, clock);
        limits.answered("api.example", 200, Some("30"), now, clock);
        assert_eq!(limits.turn("api.example", "p", None, now), Ok(now));
    }

    #[test]
    fn a_long_pause_is_refused_rather_than_waited_for_and_capped() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        limits.answered("api.example", 429, Some("86400"), now, clock);
        assert_eq!(
            limits.turn("api.example", "p", None, now),
            Err("api.example cannot be asked again for 3600 s".into())
        );
        // A refused request takes no turn, so the pause does not grow.
        assert_eq!(limits.turn("api.example", "p", None, now + MAX_PAUSE), Ok(now + MAX_PAUSE));
    }

    #[test]
    fn a_pause_that_has_passed_is_forgotten() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        limits.answered("api.example", 429, Some("1"), now, clock);
        assert_eq!(limits.turn("api.example", "p", None, now + 2 * SECOND), Ok(now + 2 * SECOND));
        assert!(limits.destinations.lock().unwrap().is_empty());
    }
}
