//! A destination's rate limit: as a manifest gives it, the problems with one, and the gap it
//! keeps between requests.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{is_host_name, unknown_keys};

/// The keys a rate limit given as an object may give.
pub(super) const LIMIT_KEYS: [&str; 2] = ["limit", "per"];

/// A destination's rate limit as a manifest gives it: `"1/s"`, which counts the plugin's own
/// requests, or `{"limit": "1/s", "per": "user"}`, which says whose it counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Limit {
    Pace(String),
    Counted { limit: String, per: Per },
}

/// Whose requests to a destination a rate limit counts (`design/plugins.md` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Per {
    /// The plugin's own, in every library and for every user.
    Plugin,
    /// The plugin's own in runs for one connected user, for a service that limits each
    /// account. A run for no user counts as the plugin's.
    User,
    /// Every plugin's, for a service that limits the whole server. Needs
    /// [`Permission::HostRateLimits`](super::Permission::HostRateLimits); without it, it counts
    /// the plugin's own.
    Host,
}

/// A rate limit a manifest declares, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimit {
    /// Lowercase, however the manifest gives it.
    pub host: String,
    /// As the manifest gives it, such as `"1/s"`.
    pub limit: String,
    /// The gap kept between requests.
    pub gap: Duration,
    pub per: Per,
}

/// The gap between requests that keeps to a rate limit: `"1/s"`, `"60/min"`, or `"1000/h"`.
/// Requests are spaced evenly rather than let through in bursts, which keeps within the limit
/// however the service counts it.
pub fn rate_limit_gap(limit: &str) -> Option<Duration> {
    let (count, per) = limit.split_once('/')?;
    let period = match per {
        "s" => 1,
        "min" => 60,
        "h" => 60 * 60,
        _ => return None,
    };
    // `parse` alone would take a sign or leading zeros. Refusing a leading zero refuses `0` too,
    // and nine digits always fit a `u32`.
    let digits = count.bytes().all(|b| b.is_ascii_digit()) && !count.starts_with('0');
    if !digits || count.is_empty() || count.len() > 9 {
        return None;
    }
    Some(Duration::from_secs(period) / count.parse::<u32>().ok()?)
}

/// Every problem with the `rateLimits` of the network request at `at`, checked against the
/// `destinations` it lists.
pub(super) fn validate_rate_limits(
    at: &str,
    limits: &Value,
    destinations: Option<&Value>,
) -> Vec<String> {
    let Some(limits) = limits.as_object() else {
        return vec![format!(
            r#"{at}: "rateLimits" must map destinations to limits such as "1/s""#
        )];
    };
    let hosts = destinations.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let listed = || hosts.iter().filter_map(Value::as_str);
    let mut problems = Vec::new();
    for (host, limit) in limits {
        // Hosts are compared without case, so two keys that differ only in case would be one.
        let earlier = limits.keys().take_while(|key| *key != host);
        if let Some(same) = earlier.into_iter().find(|key| key.eq_ignore_ascii_case(host)) {
            problems.push(format!(r#"{at}: rateLimits: "{host}" is the same host as "{same}""#));
            continue;
        }
        let (pace, per) = match limit {
            Value::Object(o) => (o.get("limit"), o.get("per")),
            _ => (Some(limit), None),
        };
        let named = listed().any(|d| d.eq_ignore_ascii_case(host));
        if !is_host_name(host) {
            problems.push(format!(
                r#"{at}: rateLimits: "{host}" is not a host name; each limit is for one host"#
            ));
        } else if !named && !listed().any(|d| d == "*") {
            problems.push(format!(r#"{at}: rateLimits: "{host}" is not one of its destinations"#));
        } else if !named && per.and_then(Value::as_str) == Some("host") {
            // A limit on every plugin is one the admin approves by name.
            problems.push(format!(
                r#"{at}: rateLimits: "{host}" limits every plugin, so its destinations must name it rather than "*""#
            ));
        }
        if pace.and_then(Value::as_str).is_none_or(|pace| rate_limit_gap(pace).is_none()) {
            problems.push(format!(
                r#"{at}: rateLimits: "{host}" needs a limit such as "1/s", "60/min", or "1000/h""#
            ));
        }
        let known = |per: &str| serde_json::from_value::<Per>(per.into()).is_ok();
        if limit.is_object() && !per.and_then(Value::as_str).is_some_and(known) {
            problems.push(format!(
                r#"{at}: rateLimits: "{host}" needs "per": "plugin", "user", or "host""#
            ));
        }
        if let Value::Object(o) = limit {
            let at = format!(r#"{at}: rateLimits: "{host}""#);
            problems.extend(unknown_keys(&at, o, &LIMIT_KEYS));
        }
    }
    problems
}
