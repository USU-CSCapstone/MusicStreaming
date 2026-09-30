//! The `http` import: a request of the plugin's choosing, to the destinations its manifest
//! names and nowhere else. A redirect is followed only to an approved destination, so an
//! approved service cannot hand the plugin on to one that was not.

use std::time::Duration;

use super::bindings::jewelcase::plugin::http;
use super::{Library, Run};
use crate::manifest::Permission;
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
        let mut builder = self.client()?.request(method(request.method), &request.url);
        for (name, value) in request.headers {
            if RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
                return Err(format!("{name} is set by the host"));
            }
            builder = builder.header(name, value);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }
        let fetch = async {
            let mut res =
                builder.send().await.map_err(|e| format!("request failed: {}", describe(e)))?;
            let status = res.status().as_u16();
            let headers = res
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    Some((name.to_string(), value.to_str().ok()?.to_owned()))
                })
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
            Ok(http::Response { status, headers, body })
        };
        tokio::time::timeout(TIMEOUT, fetch)
            .await
            .map_err(|_| "the request timed out".to_owned())?
    }
}

#[cfg(test)]
mod tests;
