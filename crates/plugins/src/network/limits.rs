//! How often each destination is asked (`requirements/plugins.md` §7). Requests to a host are
//! counted in scopes, each paced apart from the rest:
//!
//! - **A plugin's own**, unless its manifest says otherwise. No other plugin slows it.
//! - **A user's**: a plugin's requests in runs for one user who connected it, for a service
//!   that limits each account, so one user's runs do not use up another's.
//! - **Global**: those of every plugin that declared the host's limit global and was granted
//!   that, for a service that limits each address, which two plugins that each kept to it
//!   alone would break together. The strictest pace any of them declared is kept to.
//!
//! In each, requests keep to the declared pace, and a `429`, or a `503` with `Retry-After`,
//! pauses that scope's requests to the host and no other's.
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

/// Whose requests to a host are counted together.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    Plugin(String),
    /// A plugin's, in runs for one user.
    User(String, i64),
}

#[derive(Default)]
pub struct Limits {
    destinations: Mutex<HashMap<(String, Scope), Destination>>,
}

/// One scope's requests to one host.
#[derive(Default)]
struct Destination {
    /// The gap each plugin counted here declared between requests. The widest is kept to.
    declared: HashMap<String, Duration>,
    /// The earliest the next request may go.
    next: Option<Instant>,
}

impl Limits {
    /// When a request from `plugin` to `host`, counted in `scope`, may go, with that turn
    /// taken, or how long until it may if that is more than [`MAX_WAIT`]. `declared` is the gap
    /// its manifest asks for there, if any.
    pub fn turn(
        &self,
        host: &str,
        scope: &Scope,
        plugin: &str,
        declared: Option<Duration>,
        now: Instant,
    ) -> Result<Instant, Duration> {
        let key = (host.to_ascii_lowercase(), scope.clone());
        let mut destinations = self.destinations.lock().expect("limits lock");
        // Most hosts have no limit and no pause, and are never recorded.
        if declared.is_none() && !destinations.contains_key(&key) {
            return Ok(now);
        }
        let destination = destinations.entry(key.clone()).or_default();
        match declared {
            Some(gap) => destination.declared.insert(plugin.to_owned(), gap),
            None => destination.declared.remove(plugin),
        };
        let start = destination.next.map_or(now, |next| next.max(now));
        if start > now + MAX_WAIT {
            return Err(start - now);
        }
        let gap = destination.declared.values().max().copied().unwrap_or_default();
        if destination.declared.is_empty() && start == now {
            // Nothing declared and no pause left: forget it.
            destinations.remove(&key);
        } else {
            destination.next = Some(start + gap);
        }
        Ok(start)
    }

    /// What `host` answered a request counted in `scope` with. A `429`, or a `503` that says
    /// when to come back, pauses that scope's requests there. `clock` is the wall-clock time,
    /// for a `Retry-After` given as a date.
    pub fn answered(
        &self,
        host: &str,
        scope: &Scope,
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
        let destination =
            destinations.entry((host.to_ascii_lowercase(), scope.clone())).or_default();
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

    fn own(plugin: &str) -> Scope {
        Scope::Plugin(plugin.into())
    }

    #[test]
    fn a_host_nobody_limits_is_never_held_up_or_recorded() {
        let (limits, now) = (Limits::default(), Instant::now());
        for _ in 0..3 {
            assert_eq!(limits.turn("lrclib.net", &own("lyrics"), "lyrics", None, now), Ok(now));
        }
        assert!(limits.destinations.lock().unwrap().is_empty());
    }

    #[test]
    fn a_plugins_own_pace_holds_up_no_other_plugin() {
        let (limits, now) = (Limits::default(), Instant::now());
        let (host, artwork) = ("musicbrainz.org", own("artwork"));
        assert_eq!(limits.turn(host, &artwork, "artwork", Some(SECOND), now), Ok(now));
        assert_eq!(limits.turn(host, &artwork, "artwork", Some(SECOND), now), Ok(now + SECOND));
        assert_eq!(limits.turn(host, &own("biographies"), "biographies", None, now), Ok(now));
        // Hosts are compared without case.
        let host = "MusicBrainz.org";
        assert_eq!(limits.turn(host, &artwork, "artwork", Some(SECOND), now), Ok(now + 2 * SECOND));
    }

    #[test]
    fn each_user_is_paced_apart() {
        let (limits, now) = (Limits::default(), Instant::now());
        let host = "api.listenbrainz.org";
        let user = |id| Scope::User("scrobbler".into(), id);
        assert_eq!(limits.turn(host, &user(1), "scrobbler", Some(SECOND), now), Ok(now));
        assert_eq!(limits.turn(host, &user(2), "scrobbler", Some(SECOND), now), Ok(now));
        assert_eq!(limits.turn(host, &user(1), "scrobbler", Some(SECOND), now), Ok(now + SECOND));
    }

    #[test]
    fn a_global_limit_keeps_to_the_strictest_pace_any_plugin_sharing_it_declared() {
        let (limits, now) = (Limits::default(), Instant::now());
        let (host, global) = ("api.example", Scope::Global);
        let lenient = SECOND / 10;
        assert_eq!(limits.turn(host, &global, "lenient", Some(lenient), now), Ok(now));
        assert_eq!(limits.turn(host, &global, "strict", Some(2 * SECOND), now), Ok(now + lenient));
        // Once the strict plugin has declared its pace, the lenient one keeps to it too.
        assert_eq!(
            limits.turn(host, &global, "lenient", Some(lenient), now),
            Ok(now + lenient + 2 * SECOND)
        );
        // A plugin that does not share the limit is not held up by it.
        assert_eq!(limits.turn(host, &own("apart"), "apart", Some(lenient), now), Ok(now));
    }

    #[test]
    fn a_429_pauses_only_the_scope_it_was_counted_in() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        limits.answered("api.example", &own("told"), 429, Some("5"), now, clock);
        assert_eq!(
            limits.turn("api.example", &own("told"), "told", None, now),
            Ok(now + 5 * SECOND)
        );
        assert_eq!(limits.turn("api.example", &own("other"), "other", None, now), Ok(now));
        assert_eq!(limits.turn("api.example", &Scope::Global, "other", None, now), Ok(now));
        // Without a Retry-After, a 429 still pauses it.
        limits.answered("quiet.example", &Scope::Global, 429, None, now, clock);
        let turn = limits.turn("quiet.example", &Scope::Global, "other", None, now);
        assert_eq!(turn, Ok(now + DEFAULT_PAUSE));
    }

    #[test]
    fn retry_after_may_be_a_date() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        let at = httpdate::fmt_http_date(clock + 20 * SECOND);
        limits.answered("api.example", &own("p"), 503, Some(&at), now, clock);
        let turn = limits.turn("api.example", &own("p"), "p", None, now).unwrap();
        // A date is to the second.
        assert!((now + 19 * SECOND..=now + 20 * SECOND).contains(&turn));
    }

    #[test]
    fn other_answers_pause_nothing() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        limits.answered("api.example", &own("p"), 503, None, now, clock);
        limits.answered("api.example", &own("p"), 500, Some("30"), now, clock);
        limits.answered("api.example", &own("p"), 200, Some("30"), now, clock);
        assert_eq!(limits.turn("api.example", &own("p"), "p", None, now), Ok(now));
    }

    #[test]
    fn a_long_pause_is_refused_rather_than_waited_for_and_capped() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        limits.answered("api.example", &own("p"), 429, Some("86400"), now, clock);
        assert_eq!(limits.turn("api.example", &own("p"), "p", None, now), Err(MAX_PAUSE));
        // A refused request takes no turn, so the pause does not grow.
        let later = now + MAX_PAUSE;
        assert_eq!(limits.turn("api.example", &own("p"), "p", None, later), Ok(later));
    }

    #[test]
    fn a_pause_that_has_passed_is_forgotten() {
        let (limits, now, clock) = (Limits::default(), Instant::now(), SystemTime::now());
        limits.answered("api.example", &own("p"), 429, Some("1"), now, clock);
        let later = now + 2 * SECOND;
        assert_eq!(limits.turn("api.example", &own("p"), "p", None, later), Ok(later));
        assert!(limits.destinations.lock().unwrap().is_empty());
    }
}
