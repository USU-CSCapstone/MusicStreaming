//! Replies kept for reuse, so that a question already answered is not asked again while its
//! answer is fresh, whichever plugin asks it (`requirements/plugins.md` §2.1).
//!
//! It is a shared cache as RFC 9111 describes one. It keeps a reply only for as long as the
//! reply's own `Cache-Control` or `Expires` allows, up to a day, and asks again with its `ETag`
//! or `Last-Modified` once that runs out. It keeps nothing that says `no-store` or `private`,
//! or that sets a cookie. It is held in memory, so a restart empties it. A plugin keeps what
//! must last in its `state`.
//!
//! A reply is reused only for a request with the same library, URL, and every header the
//! plugin sent. So a reply fetched with a credential, in a header or in the URL, is reused
//! only by a request carrying that same credential. Nothing fetched for one library is reused
//! for another, because whether a reply was already cached would tell a plugin something about
//! a library it cannot read (`requirements/general.md` §3.6).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// The largest reply kept.
const MAX_ENTRY: usize = 2 << 20;
/// All replies kept together. The least recently used goes first.
const MAX_SIZE: usize = 32 << 20;
/// The longest a reply is reused for without asking again, whatever it says.
const MAX_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
/// Statuses kept (RFC 9110 §15.1). A 404 is among them, so "there is nothing here" is asked
/// once too.
const STATUSES: [u16; 7] = [200, 203, 204, 300, 301, 404, 410];
/// Request headers by which a plugin manages caching itself. The cache stays out of a request
/// carrying any of them.
const OWN_CACHING: [&str; 8] = [
    "cache-control",
    "pragma",
    "if-match",
    "if-none-match",
    "if-modified-since",
    "if-unmodified-since",
    "if-range",
    "range",
];

/// Which request a reply answers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    library: i64,
    url: String,
    /// Every header the plugin sent, names in lowercase, sorted.
    headers: Vec<(String, String)>,
}

impl Key {
    /// The key for a GET of `url` in `library`, or none if the request manages its own
    /// caching. Only a GET may be cached; the caller checks the method.
    pub fn new(library: i64, url: &str, headers: &[(String, String)]) -> Option<Key> {
        let mut headers: Vec<_> = headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .collect();
        if headers.iter().any(|(name, _)| OWN_CACHING.contains(&name.as_str())) {
            return None;
        }
        headers.sort();
        Some(Key { library, url: url.to_owned(), headers })
    }

    fn size(&self) -> usize {
        self.url.len() + self.headers.iter().map(|(n, v)| n.len() + v.len()).sum::<usize>()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    /// Names in lowercase.
    pub headers: Vec<(String, String)>,
    /// Shared with the cache, so keeping a reply copies nothing.
    pub body: Arc<Vec<u8>>,
    /// Where it came from after redirects, which a later request must be allowed to reach.
    pub url: String,
}

impl Reply {
    /// The value of header `name`, given in lowercase.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    fn size(&self) -> usize {
        self.body.len()
            + self.url.len()
            + self.headers.iter().map(|(n, v)| n.len() + v.len()).sum::<usize>()
    }
}

pub enum Lookup {
    /// Fresh: use it without asking.
    Fresh(Reply),
    /// No longer fresh, but it can be checked with [`conditions`] rather than fetched whole.
    Stale(Reply),
    Miss,
}

#[derive(Default)]
pub struct Cache {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<Key, Entry>,
    /// Each entry by when it was last used, oldest first.
    used: BTreeMap<u64, Key>,
    tick: u64,
    size: usize,
}

struct Entry {
    reply: Reply,
    fresh_until: Instant,
    used: u64,
    size: usize,
}

impl Cache {
    pub fn lookup(&self, key: &Key, now: Instant) -> Lookup {
        let mut inner = self.inner.lock().expect("cache lock");
        let Some(entry) = inner.entries.get(key) else { return Lookup::Miss };
        let (fresh, reply) = (now < entry.fresh_until, entry.reply.clone());
        if !fresh && conditions(&reply).is_empty() {
            inner.remove(key);
            return Lookup::Miss;
        }
        inner.touch(key);
        if fresh { Lookup::Fresh(reply) } else { Lookup::Stale(reply) }
    }

    /// Keeps `reply` for `key` if it says it may be kept, and otherwise forgets anything
    /// kept for `key` before. `clock` is the wall-clock time, for `Expires`.
    pub fn store(&self, key: Key, reply: Reply, now: Instant, clock: SystemTime) {
        let mut inner = self.inner.lock().expect("cache lock");
        inner.remove(&key);
        let Some(lifetime) = lifetime(&reply, clock) else { return };
        let size = key.size() + reply.size();
        inner.tick += 1;
        let used = inner.tick;
        inner.used.insert(used, key.clone());
        inner.size += size;
        inner.entries.insert(key, Entry { reply, fresh_until: now + lifetime, used, size });
        while inner.size > MAX_SIZE {
            let Some((_, oldest)) = inner.used.pop_first() else { break };
            if let Some(entry) = inner.entries.remove(&oldest) {
                inner.size -= entry.size;
            }
        }
    }

    /// The service answered a check of `stale` with `304 Not Modified` and these headers: it
    /// is fresh again, with the headers the `304` updated (RFC 9111 §4.3.4).
    pub fn refresh(
        &self,
        key: Key,
        stale: Reply,
        updated: Vec<(String, String)>,
        now: Instant,
        clock: SystemTime,
    ) -> Reply {
        let mut headers: Vec<_> = stale
            .headers
            .into_iter()
            .filter(|(name, _)| !updated.iter().any(|(n, _)| n == name))
            .collect();
        // Content-Length describes the 304's empty body, not the one kept.
        headers.extend(updated.into_iter().filter(|(name, _)| name != "content-length"));
        let reply = Reply { headers, ..stale };
        self.store(key, reply.clone(), now, clock);
        reply
    }
}

impl Inner {
    fn remove(&mut self, key: &Key) {
        if let Some(entry) = self.entries.remove(key) {
            self.used.remove(&entry.used);
            self.size -= entry.size;
        }
    }

    fn touch(&mut self, key: &Key) {
        self.tick += 1;
        let tick = self.tick;
        if let Some(entry) = self.entries.get_mut(key) {
            self.used.remove(&entry.used);
            entry.used = tick;
            self.used.insert(tick, key.clone());
        }
    }
}

/// The headers that ask whether `stale` is still current: none if it has no validator.
pub fn conditions(stale: &Reply) -> Vec<(String, String)> {
    let mut conditions = Vec::new();
    if let Some(etag) = stale.header("etag") {
        conditions.push(("if-none-match".to_owned(), etag.to_owned()));
    }
    if let Some(modified) = stale.header("last-modified") {
        conditions.push(("if-modified-since".to_owned(), modified.to_owned()));
    }
    conditions
}

/// How long `reply` may be reused without asking again, or none if it may not be kept. Zero
/// is worth keeping when it has a validator, since checking it costs less than fetching it.
fn lifetime(reply: &Reply, clock: SystemTime) -> Option<Duration> {
    if !STATUSES.contains(&reply.status) || reply.body.len() > MAX_ENTRY {
        return None;
    }
    let all = |name: &'static str| {
        reply.headers.iter().filter(move |(n, _)| n == name).map(|(_, v)| v.as_str())
    };
    if reply.header("set-cookie").is_some() || all("vary").any(|v| v.trim() == "*") {
        return None;
    }
    let directives: Vec<(String, Option<&str>)> = all("cache-control")
        .flat_map(|value| value.split(','))
        .map(|directive| match directive.split_once('=') {
            Some((name, value)) => {
                (name.trim().to_ascii_lowercase(), Some(value.trim().trim_matches('"')))
            }
            None => (directive.trim().to_ascii_lowercase(), None),
        })
        .collect();
    let has = |name: &str| directives.iter().any(|(n, _)| n == name);
    let seconds = |name: &str| {
        let value = directives.iter().find(|(n, _)| n == name).and_then(|(_, v)| *v);
        value.and_then(|v| v.parse::<u64>().ok()).map(Duration::from_secs)
    };
    if has("no-store") || has("private") {
        return None;
    }
    let date = |value: Option<&str>| value.and_then(|v| httpdate::parse_http_date(v).ok());
    let lifetime = if has("no-cache") {
        Duration::ZERO
    } else if let Some(max_age) = seconds("s-maxage").or_else(|| seconds("max-age")) {
        max_age
    } else if let Some(expires) = reply.header("expires") {
        // An `Expires` that is not a date, such as `0`, means already expired.
        let sent = date(reply.header("date")).unwrap_or(clock);
        let expires = date(Some(expires));
        expires.and_then(|at| at.duration_since(sent).ok()).unwrap_or_default()
    } else {
        Duration::ZERO
    };
    let age = reply.header("age").and_then(|a| a.trim().parse().ok()).map(Duration::from_secs);
    let lifetime = lifetime.saturating_sub(age.unwrap_or_default()).min(MAX_LIFETIME);
    let validated = reply.header("etag").is_some() || reply.header("last-modified").is_some();
    (lifetime > Duration::ZERO || validated).then_some(lifetime)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    fn key(url: &str) -> Key {
        Key::new(1, url, &[]).unwrap()
    }

    fn reply(status: u16, headers: &[(&str, &str)]) -> Reply {
        Reply {
            status,
            headers: headers.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
            body: Arc::new(b"hello".to_vec()),
            url: "https://api.example/".into(),
        }
    }

    fn fresh(lookup: Lookup) -> Option<Reply> {
        match lookup {
            Lookup::Fresh(reply) => Some(reply),
            _ => None,
        }
    }

    #[test]
    fn a_reply_is_reused_while_fresh_and_no_longer() {
        let (cache, now, clock) = (Cache::default(), Instant::now(), SystemTime::now());
        let kept = reply(200, &[("cache-control", "public, max-age=60")]);
        cache.store(key("https://api.example/a"), kept.clone(), now, clock);
        assert_eq!(fresh(cache.lookup(&key("https://api.example/a"), now)), Some(kept));
        assert!(matches!(cache.lookup(&key("https://api.example/b"), now), Lookup::Miss));
        let later = now + 61 * SECOND;
        assert!(matches!(cache.lookup(&key("https://api.example/a"), later), Lookup::Miss));
    }

    #[test]
    fn nothing_is_kept_that_says_not_to_be_or_says_nothing() {
        let (now, clock) = (Instant::now(), SystemTime::now());
        for headers in [
            &[][..],
            &[("cache-control", "no-store")],
            &[("cache-control", "private, max-age=60")],
            &[("cache-control", "max-age=60"), ("set-cookie", "a=b")],
            &[("cache-control", "max-age=60"), ("vary", "*")],
            &[("cache-control", "max-age=60"), ("age", "60")],
            &[("expires", "0")],
        ] {
            let cache = Cache::default();
            cache.store(key("https://api.example/"), reply(200, headers), now, clock);
            assert!(matches!(cache.lookup(&key("https://api.example/"), now), Lookup::Miss));
        }
        let cache = Cache::default();
        let error = reply(500, &[("cache-control", "max-age=60")]);
        cache.store(key("https://api.example/"), error, now, clock);
        assert!(matches!(cache.lookup(&key("https://api.example/"), now), Lookup::Miss));
    }

    #[test]
    fn freshness_follows_s_maxage_then_max_age_then_expires_less_age() {
        let clock = SystemTime::now();
        let at = |seconds| httpdate::fmt_http_date(clock + seconds * SECOND);
        let expires = at(100);
        let sent = httpdate::fmt_http_date(clock);
        for (headers, lifetime) in [
            (vec![("cache-control", "max-age=10, s-maxage=20")], 20),
            (vec![("cache-control", "max-age=\"10\""), ("expires", expires.as_str())], 10),
            (vec![("expires", expires.as_str()), ("date", sent.as_str())], 100),
            (vec![("cache-control", "max-age=30"), ("age", "10")], 20),
            (vec![("cache-control", "max-age=31536000")], MAX_LIFETIME.as_secs()),
        ] {
            let reply = reply(200, &headers);
            let found = super::lifetime(&reply, clock).unwrap().as_secs();
            // A date is to the second.
            assert!((lifetime - 1..=lifetime).contains(&found), "{headers:?}: {found}");
        }
    }

    #[test]
    fn a_stale_reply_with_a_validator_is_checked_rather_than_fetched() {
        let (cache, now, clock) = (Cache::default(), Instant::now(), SystemTime::now());
        let kept = reply(200, &[("cache-control", "no-cache"), ("etag", "\"v1\"")]);
        cache.store(key("https://api.example/"), kept, now, clock);
        let Lookup::Stale(stale) = cache.lookup(&key("https://api.example/"), now) else {
            panic!("a reply with an ETag is kept to be checked");
        };
        assert_eq!(conditions(&stale), [("if-none-match".to_owned(), "\"v1\"".to_owned())]);
        let updated = vec![("cache-control".into(), "max-age=60".into())];
        let refreshed = cache.refresh(key("https://api.example/"), stale, updated, now, clock);
        assert_eq!(refreshed.header("cache-control"), Some("max-age=60"));
        assert_eq!(refreshed.header("etag"), Some("\"v1\""));
        assert_eq!(fresh(cache.lookup(&key("https://api.example/"), now)), Some(refreshed));
    }

    #[test]
    fn requests_differing_in_library_or_headers_share_nothing() {
        let (cache, now, clock) = (Cache::default(), Instant::now(), SystemTime::now());
        let token = [("Authorization".to_owned(), "Token mine".to_owned())];
        let mine = Key::new(1, "https://api.example/", &token).unwrap();
        cache.store(mine.clone(), reply(200, &[("cache-control", "max-age=60")]), now, clock);
        assert!(fresh(cache.lookup(&mine, now)).is_some());
        let theirs = [("authorization".to_owned(), "Token theirs".to_owned())];
        for other in [
            Key::new(1, "https://api.example/", &theirs).unwrap(),
            Key::new(1, "https://api.example/", &[]).unwrap(),
            Key::new(2, "https://api.example/", &token).unwrap(),
        ] {
            assert!(matches!(cache.lookup(&other, now), Lookup::Miss), "{other:?}");
        }
        // Header names are compared without case.
        let same = [("authorization".to_owned(), "Token mine".to_owned())];
        assert!(
            fresh(cache.lookup(&Key::new(1, "https://api.example/", &same).unwrap(), now))
                .is_some()
        );
    }

    #[test]
    fn a_request_that_manages_its_own_caching_is_left_alone() {
        for name in ["Cache-Control", "If-None-Match", "Range"] {
            let headers = [(name.to_owned(), "x".to_owned())];
            assert_eq!(Key::new(1, "https://api.example/", &headers), None);
        }
    }

    #[test]
    fn the_least_recently_used_goes_when_it_is_full() {
        let (cache, now, clock) = (Cache::default(), Instant::now(), SystemTime::now());
        let large = |n: u8| Reply {
            body: Arc::new(vec![n; MAX_ENTRY]),
            ..reply(200, &[("cache-control", "max-age=60")])
        };
        // One fewer than fills it, since keys and headers take room too.
        let count = MAX_SIZE / MAX_ENTRY - 1;
        for n in 0..count {
            cache.store(key(&format!("https://api.example/{n}")), large(n as u8), now, clock);
        }
        // Using the first makes the second the least recently used.
        assert!(fresh(cache.lookup(&key("https://api.example/0"), now)).is_some());
        cache.store(key("https://api.example/new"), large(0), now, clock);
        assert!(fresh(cache.lookup(&key("https://api.example/0"), now)).is_some());
        assert!(matches!(cache.lookup(&key("https://api.example/1"), now), Lookup::Miss));
        assert!(fresh(cache.lookup(&key("https://api.example/new"), now)).is_some());
        assert!(cache.inner.lock().unwrap().size <= MAX_SIZE);
    }
}
