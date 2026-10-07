//! The `http` import: a request of the plugin's choosing, to the destinations its manifest
//! names and nowhere else. A redirect is followed only to an approved destination, so an
//! approved service cannot hand the plugin on to one that was not.
//!
//! Every plugin's requests share two things (`crate::network`). Each waits its turn at the
//! pace its destination allows. A GET is answered from a reply already fetched while that
//! reply is fresh, and a stale one is checked rather than fetched again.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use super::bindings::jewelcase::plugin::http;
use super::{Library, Run};
use crate::manifest::Permission;
use crate::network::cache::{self, Key, Lookup, Reply};
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
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error("it redirected too many times");
            }
            match rules::allowed(attempt.url().as_str(), &destinations) {
                Ok(()) => attempt.follow(),
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
    async fn send(&mut self, request: http::Request) -> Result<http::Response, String> {
        self.may(Permission::Network)?;
        rules::allowed(&request.url, &self.grants.destinations)?;
        if request.body.len() > MAX_BODY {
            return Err(format!("a request body is at most {} MB", MAX_BODY >> 20));
        }
        let reserved = |name: &String| RESERVED.contains(&name.to_ascii_lowercase().as_str());
        if let Some((name, _)) = request.headers.iter().find(|(name, _)| reserved(name)) {
            return Err(format!("{name} is set by the host"));
        }
        let key = matches!(request.method, http::Method::Get)
            .then(|| Key::new(self.grants.library, &request.url, &request.headers))
            .flatten();
        let mut stale = None;
        if let Some(key) = &key {
            match self.network.cache.lookup(key, Instant::now()) {
                Lookup::Fresh(reply) => return self.reuse(reply),
                Lookup::Stale(reply) => stale = Some(reply),
                Lookup::Miss => {}
            }
        }
        // `allowed` has parsed it, so it has a host.
        let host = reqwest::Url::parse(&request.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        let limits = &self.grants.rate_limits;
        let declared = limits.iter().find(|(d, _)| d.eq_ignore_ascii_case(&host)).map(|l| l.1);
        let turn =
            self.network.limits.turn(&host, &self.grants.plugin, declared, Instant::now())?;
        tokio::time::sleep_until(turn.into()).await;

        let mut builder = self.client()?.request(method(request.method), &request.url);
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
            .map_err(|_| "the request timed out".to_owned())??;
        let (now, clock) = (Instant::now(), SystemTime::now());
        let retry_after = reply.header("retry-after");
        self.network.limits.answered(&host, reply.status, retry_after, now, clock);
        let Some(key) = key else { return Ok(response(reply)) };
        let reply = match stale {
            Some(stale) if reply.status == 304 => {
                self.network.cache.refresh(key, stale, reply.headers, now, clock)
            }
            _ => {
                self.network.cache.store(key, reply.clone(), now, clock);
                reply
            }
        };
        Ok(response(reply))
    }
}

impl<L: Library> Run<L> {
    /// A kept reply, if this plugin may reach where it came from: one that was redirected is
    /// refused to a plugin that could not have followed the redirect itself.
    fn reuse(&self, reply: Reply) -> Result<http::Response, String> {
        rules::allowed(&reply.url, &self.grants.destinations)
            .map_err(|refused| format!("request failed: it redirected elsewhere: {refused}"))?;
        Ok(response(reply))
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
