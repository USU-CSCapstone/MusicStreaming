//! The `http` import: a request of the plugin's choosing, to the destinations its manifest
//! names and nowhere else. A redirect is followed only to an approved destination, so an
//! approved service cannot hand the plugin on to one that was not.
//!
//! Every plugin's requests share two things (`crate::network`). Each waits its turn at the
//! pace its destination allows, in the queues its manifest and the host's limits say. A GET is
//! answered from a reply already fetched while that reply is fresh, and a stale one is checked
//! rather than fetched again.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use super::bindings::jewelcase::plugin::http;
use super::{Library, Run};
use crate::manifest::{Per, Permission};
use crate::network::cache::{self, Key, Lookup, Reply, Visible};
use crate::network::limits::Scope;
use crate::rules;

const TIMEOUT: Duration = Duration::from_secs(15);
/// The largest request or reply body. Either is held in the plugin's memory whole.
const MAX_BODY: usize = 16 << 20;
const MAX_REDIRECTS: usize = 5;
const USER_AGENT: &str = concat!("Jewelcase/", env!("CARGO_PKG_VERSION"), " (plugin host)");

/// Headers the host sets. `host` above all: it would let a request name one site to the
/// destination check and reach another at the same address.
const RESERVED: [&str; 7] =
    ["host", "content-length", "transfer-encoding", "connection", "upgrade", "te", "trailer"];

impl<L: Library> Run<L> {
    /// Made on the first request, so a plugin that never uses the network costs no client.
    fn client(&mut self) -> Result<reqwest::Client, String> {
        if let Some(client) = &self.client {
            return Ok(client.clone());
        }
        let destinations = self.grants.destinations.clone();
        let redirects = reqwest::redirect::Policy::custom(move |attempt| {
            // The first of `previous` is where the request started, not a redirect.
            if attempt.previous().len() > MAX_REDIRECTS {
                return attempt.error("it redirected too many times");
            }
            match rules::allowed(attempt.url().as_str(), &destinations) {
                Ok(_) => attempt.follow(),
                Err(refused) => attempt.error(format!("it redirected elsewhere: {refused}")),
            }
        });
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .redirect(redirects)
            .build()
            .map_err(|e| format!("the network is unavailable: {e}"))?;
        Ok(self.client.insert(client).clone())
    }
}

fn method(method: http::Method) -> reqwest::Method {
    match method {
        http::Method::Get => reqwest::Method::GET,
        http::Method::Head => reqwest::Method::HEAD,
        http::Method::Post => reqwest::Method::POST,
        http::Method::Put => reqwest::Method::PUT,
        http::Method::Patch => reqwest::Method::PATCH,
        http::Method::Delete => reqwest::Method::DELETE,
    }
}

/// A failure and what caused it, without the URL, which the plugin already knows.
fn describe(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut message = error.to_string();
    let mut cause = std::error::Error::source(&error);
    while let Some(inner) = cause {
        message.push_str(": ");
        message.push_str(&inner.to_string());
        cause = inner.source();
    }
    message
}

impl<L: Library> http::Host for Run<L> {
    async fn send(&mut self, request: http::Request) -> Result<http::Response, http::Error> {
        self.may(Permission::Network)?;
        let url = rules::allowed(&request.url, &self.grants.destinations)
            .map_err(http::Error::Refused)?;
        // Checked to have one. Parsed, so it is lowercase, and every spelling of a URL is one.
        let host = url.host_str().unwrap_or_default().to_owned();
        if request.body.len() > MAX_BODY {
            let refused = format!("a request body is at most {} MB", MAX_BODY >> 20);
            return Err(http::Error::Refused(refused));
        }
        let reserved = |name: &String| RESERVED.contains(&name.to_ascii_lowercase().as_str());
        if let Some((name, _)) = request.headers.iter().find(|(name, _)| reserved(name)) {
            return Err(http::Error::Refused(format!("{name} is set by the host")));
        }
        let changes = matches!(
            request.method,
            http::Method::Post | http::Method::Put | http::Method::Patch | http::Method::Delete
        );
        let key = (matches!(request.method, http::Method::Get) && request.body.is_empty())
            .then(|| Key::new(self.visible(), url.as_str(), &request.headers))
            .flatten();
        let mut stale = None;
        if let Some(key) = &key {
            match self.network.cache.lookup(key, Instant::now()) {
                Lookup::Fresh(reply) => return self.reuse(reply),
                // One it could not reuse is no use checking either.
                Lookup::Stale(reply) => stale = self.may_reuse(&reply).is_ok().then_some(reply),
                Lookup::Miss => {}
            }
        }
        let turn = self.network.limits.turn(&host, &self.queues(&host), Instant::now());
        // Rounded up, so a plugin that waits that long finds its turn has come.
        let turn = turn.map_err(|wait| {
            let ms = wait.as_micros().div_ceil(1000);
            http::Error::RateLimited(u64::try_from(ms).unwrap_or(u64::MAX))
        })?;
        tokio::time::sleep_until(turn.into()).await;

        let client = self.client().map_err(http::Error::Failed)?;
        let mut builder = client.request(method(request.method), url.clone());
        let conditions = stale.as_ref().map(cache::conditions).unwrap_or_default();
        for (name, value) in request.headers.into_iter().chain(conditions) {
            builder = builder.header(name, value);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }
        // The wait for a turn is not the service's slowness, so only the request is timed.
        let reply = tokio::time::timeout(TIMEOUT, fetch(builder))
            .await
            .map_err(|_| http::Error::TimedOut)?
            .map_err(http::Error::Failed)?;
        let (now, clock) = (Instant::now(), SystemTime::now());
        let retry_after = reply.header("retry-after");
        // Redirects have been followed, so the answer is from where the last one led.
        let answered = rules::host_of(&reply.url);
        let (scope, _) = self.own(&answered);
        self.network.limits.answered(&answered, &scope, reply.status, retry_after, now, clock);
        // After a redirect, the next request to where this one started would be sent on into the
        // same throttle, so it waits there too.
        if answered != host && matches!(reply.status, 429 | 503) {
            let (scope, _) = self.own(&host);
            self.network.limits.answered(&host, &scope, reply.status, retry_after, now, clock);
        }
        // A change that succeeded leaves what was kept for its URL out of date (RFC 9111 §4.4).
        if changes && reply.status < 400 {
            self.network.cache.invalidate(url.as_str());
        }
        let Some(key) = key else { return Ok(response(reply)) };
        let reply = match stale {
            Some(stale) if reply.status == 304 => {
                self.network.cache.refresh(key, stale, reply.headers, now, clock)
            }
            // Never kept, and storing one would forget what was: a check that failed keeps it,
            // and its validator (RFC 9111 §4.3.3), and so does a request that could not check
            // it, for other runs that still can.
            _ if reply.status == 429 || reply.status >= 500 => reply,
            _ => {
                self.network.cache.store(key, reply.clone(), now, clock);
                reply
            }
        };
        Ok(response(reply))
    }
}

impl<L: Library> Run<L> {
    /// The queues a request to `host` waits in, each with the gap it keeps: its own scope's,
    /// and the host's if a limit there binds this plugin.
    fn queues(&self, host: &str) -> Vec<(Scope, Duration)> {
        let own = self.own(host);
        let shared = self.host_limit(host).map(|gap| (Scope::Host, gap));
        let shared = shared.filter(|(scope, _)| *scope != own.0);
        [own].into_iter().chain(shared).collect()
    }

    /// The scope this run's requests to `host` count in, as its manifest says, and the gap kept
    /// there. A user's limit in a run for no user, or a host's limit the admin did not allow
    /// or exempted it from, counts the plugin's own. A host's limit keeps at least the plugin's
    /// own gap, which may not be in force yet, such as in a check before it is enabled.
    fn own(&self, host: &str) -> (Scope, Duration) {
        let grants = &self.grants;
        let declared = grants.rate_limits.iter().find(|l| l.host == host);
        let gap = declared.map(|l| l.gap).unwrap_or_default();
        let allowed = grants.permissions.contains(&Permission::HostRateLimits);
        match (declared.map(|l| l.per), grants.user, self.host_limit(host)) {
            (Some(Per::Host), _, Some(host_gap)) if allowed => (Scope::Host, host_gap.max(gap)),
            (Some(Per::User), Some(user), _) => (Scope::User(grants.plugin.clone(), user), gap),
            _ => (Scope::Plugin(grants.plugin.clone()), gap),
        }
    }

    /// The gap every plugin's requests to `host` keep, if a limit there binds this plugin.
    fn host_limit(&self, host: &str) -> Option<Duration> {
        let limits = &self.grants.host_limits;
        limits.iter().find(|(limited, _)| limited == host).map(|(_, gap)| *gap)
    }

    /// A kept reply, if this plugin may reach where it came from.
    fn reuse(&self, reply: Reply) -> Result<http::Response, http::Error> {
        self.may_reuse(&reply).map_err(|refused| {
            http::Error::Failed(format!("request failed: it redirected elsewhere: {refused}"))
        })?;
        Ok(response(reply))
    }

    /// Whether this plugin may reach where a kept reply came from. One that was redirected is
    /// not for a plugin that could not have followed the redirect itself.
    fn may_reuse(&self, reply: &Reply) -> Result<(), String> {
        rules::allowed(&reply.url, &self.grants.destinations).map(|_| ())
    }

    /// What this run can see, which decides whose kept replies it shares.
    fn visible(&self) -> Visible {
        let grants = &self.grants;
        Visible {
            library: grants.library,
            user: grants.user,
            library_read: grants.permissions.contains(&Permission::LibraryRead),
            listening_activity: grants.permissions.contains(&Permission::ListeningActivity),
            search_activity: grants.permissions.contains(&Permission::SearchActivity),
        }
    }
}

/// Sends the request and reads the whole reply.
async fn fetch(builder: reqwest::RequestBuilder) -> Result<Reply, String> {
    let mut res = builder.send().await.map_err(|e| format!("request failed: {}", describe(e)))?;
    let status = res.status().as_u16();
    let url = res.url().to_string();
    // Names come lowercase from the HTTP library.
    let headers = res
        .headers()
        .iter()
        .filter_map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_owned())))
        .collect();
    let mut body = Vec::new();
    while let Some(chunk) =
        res.chunk().await.map_err(|e| format!("reply failed: {}", describe(e)))?
    {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY {
            return Err(format!("the reply is over {} MB", MAX_BODY >> 20));
        }
    }
    Ok(Reply { status, headers, body: Arc::new(body), url })
}

/// The reply as the plugin receives it. The body is copied only if the cache kept it too.
fn response(reply: Reply) -> http::Response {
    let body = Arc::try_unwrap(reply.body).unwrap_or_else(|kept| kept.as_ref().clone());
    http::Response { status: reply.status, headers: reply.headers, body }
}

#[cfg(test)]
mod tests;
