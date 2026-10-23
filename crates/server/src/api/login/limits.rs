//! Sign-in limits (`requirements/users.md` §3.1): past a few failures, each further attempt
//! waits twice as long as the one before, up to a cap, per account and per origin. The wait
//! always ends by itself, and failures older than the window stop counting.
//!
//! Limits count failures in `login_attempts` by the name tried, so an unknown username is
//! limited exactly like a real one. A success clears the account's failures but not the
//! origin's, so a sweep cannot reset its own count by signing into an account it holds.
//!
//! At an origin the account has signed in from, while that success is kept (30 days,
//! `crate::db::retention`), the account's limit counts only that origin's failures, so failures
//! from elsewhere cannot keep its owner out there.

use std::net::{IpAddr, Ipv6Addr};

use rusqlite::{Connection, params};

/// Failures further back than this do not count.
const WINDOW_MS: i64 = 60 * 60 * 1000;

/// The longest any wait gets, so a lockout always clears within this.
const MAX_WAIT_MS: i64 = 15 * 60 * 1000;

/// Failures before an account starts waiting: enough for a person unsure of their password.
const ACCOUNT_FREE: i64 = 5;

/// Failures before an origin starts waiting. More, since a household shares one.
const ORIGIN_FREE: i64 = 20;

/// Milliseconds until `username` may try again from `origin`, or 0 if it may now.
pub fn wait_ms(
    conn: &Connection,
    username: &str,
    origin: IpAddr,
    now: i64,
) -> rusqlite::Result<i64> {
    let (since, key) = (now - WINDOW_MS, origin_key(origin));
    // At an origin the account has signed in from, only that origin's failures count; with
    // `None`, failures from every origin do.
    let only_from = signed_in_from(conn, username, &key)?.then_some(&key);
    let account = conn
        .prepare_cached(
            "SELECT count(*), max(attempted_at) FROM login_attempts \
             WHERE username_attempted = ?1 AND succeeded = 0 AND (?3 IS NULL OR origin = ?3) \
             AND attempted_at > coalesce( \
             (SELECT max(attempted_at) FROM login_attempts \
              WHERE username_attempted = ?1 AND succeeded = 1 AND attempted_at > ?2), ?2)",
        )?
        .query_row(params![username, since, only_from], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let origin = conn
        .prepare_cached(
            "SELECT count(*), max(attempted_at) FROM login_attempts \
             WHERE origin = ?1 AND succeeded = 0 AND attempted_at > ?2",
        )?
        .query_row(params![key, since], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(wait(account, ACCOUNT_FREE, now).max(wait(origin, ORIGIN_FREE, now)))
}

/// Whether the active account `username` names has a kept successful sign-in from `origin`, an
/// [`origin_key`]. A suspended account, or a name whose earlier holder signed in there, counts
/// as never having signed in, so it waits as an unknown name does (`requirements/users.md` §10).
fn signed_in_from(conn: &Connection, username: &str, origin: &str) -> rusqlite::Result<bool> {
    conn.prepare_cached(
        "SELECT EXISTS (SELECT 1 FROM login_attempts WHERE origin = ?2 AND succeeded = 1 \
         AND user_id = (SELECT id FROM users WHERE username = ?1 AND status = 'active'))",
    )?
    .query_row(params![username, origin], |row| row.get(0))
}

/// Records an attempt as failed until it proves otherwise, and returns its ID. Recorded before
/// the password is checked, so attempts made at once all count against each other.
pub fn record(
    conn: &Connection,
    username: &str,
    user: Option<i64>,
    origin: IpAddr,
    now: i64,
) -> rusqlite::Result<i64> {
    conn.prepare_cached(
        "INSERT INTO login_attempts \
         (attempted_at, username_attempted, user_id, origin, succeeded) \
         VALUES (?1, ?2, ?3, ?4, 0) RETURNING id",
    )?
    .query_row(params![now, username, user, origin_key(origin)], |row| row.get(0))
}

pub fn succeeded(conn: &Connection, attempt: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE login_attempts SET succeeded = 1 WHERE id = ?1", [attempt]).map(drop)
}

/// What an origin's attempts are counted and stored under: an IPv4 address as itself, an
/// IPv4-mapped IPv6 address as that IPv4 address, and any other IPv6 address as its /64, such
/// as `2001:db8:1:2::/64`, since one client usually holds a whole /64.
fn origin_key(origin: IpAddr) -> String {
    match origin.to_canonical() {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("{}/64", Ipv6Addr::from_bits(ip.to_bits() & (u128::MAX << 64))),
    }
}

/// The wait after `failures`, the latest at `latest`: none for the first `free`, then 1 s,
/// doubling with each failure up to [`MAX_WAIT_MS`].
fn wait((failures, latest): (i64, Option<i64>), free: i64, now: i64) -> i64 {
    let Some(latest) = latest.filter(|_| failures >= free) else { return 0 };
    let doublings = (failures - free).clamp(0, 20);
    let wait = (1000_i64 << doublings).min(MAX_WAIT_MS);
    (latest + wait - now).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_double_past_the_free_failures_up_to_the_cap() {
        let at = |failures| wait((failures, Some(0)), 5, 0);
        assert_eq!([at(0), at(4), at(5), at(6), at(7), at(14)], [0, 0, 1000, 2000, 4000, 512_000]);
        assert_eq!([at(15), at(1000)], [MAX_WAIT_MS, MAX_WAIT_MS]);
    }

    #[test]
    fn a_wait_runs_from_the_latest_failure() {
        assert_eq!(wait((6, Some(10_000)), 5, 10_500), 1500);
        assert_eq!(wait((6, Some(10_000)), 5, 12_000), 0);
    }

    #[test]
    fn ipv6_origins_count_by_their_64() {
        let key = |origin: &str| origin_key(origin.parse().unwrap());
        assert_eq!(key("203.0.113.5"), "203.0.113.5");
        assert_eq!(key("::ffff:203.0.113.5"), "203.0.113.5");
        assert_eq!(key("2001:db8:1:2:3:4:5:6"), "2001:db8:1:2::/64");
        assert_eq!(key("2001:db8:1:2::"), "2001:db8:1:2::/64");
        assert_eq!(key("2001:db8:1:3::1"), "2001:db8:1:3::/64");
    }
}
