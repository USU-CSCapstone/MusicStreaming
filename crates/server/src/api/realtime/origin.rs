//! Whether a realtime connection comes from this server's own pages.

use axum::http::uri::Authority;
use axum::http::{HeaderMap, header};

use crate::api::origin::Origin;

/// Set by proxies that change `Host` (`requirements/deployment.md` §6).
const FORWARDED_HOST: &str = "x-forwarded-host";

/// Whether the request is not from a page, which sends no `Origin`, or is from a page this
/// server served: one whose origin names the host and port the request was sent to, as `Host`
/// or a trusted proxy's `X-Forwarded-Host` (`from`) says.
pub(super) fn same_origin(headers: &HeaderMap, from: &Origin) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else { return true };
    let origin = origin.to_str().ok().and_then(|origin| origin.split_once("://"));
    let Some((scheme, origin)) = origin else { return false };
    let Ok(origin) = origin.parse::<Authority>() else { return false };
    // A port left out, by the page or a proxy, is the default for the page's scheme. A proxy on
    // another port that drops it refuses every page, rather than letting in another server's.
    let default = match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    };
    let port = origin.port_u16().or(default);
    let host = headers.get(header::HOST).and_then(|host| host.to_str().ok());
    // A proxy that lists several hosts puts the one the client asked for first.
    let forwarded = from.forwarded(headers, FORWARDED_HOST).next();
    let hosts = [host, forwarded].into_iter().flatten();
    hosts.filter_map(|host| host.parse::<Authority>().ok()).any(|host| {
        host.host().eq_ignore_ascii_case(origin.host()) && host.port_u16().or(default) == port
    })
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;
    use crate::config::TrustedProxies;

    /// Whether a page at `origin` may connect from `peer`, sent to `host` with `forwarded` as its
    /// `X-Forwarded-Host`.
    fn allowed(
        peer: &str,
        origin: &'static str,
        host: &'static str,
        forwarded: &'static str,
    ) -> bool {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static(origin));
        headers.insert(header::HOST, HeaderValue::from_static(host));
        headers.insert(FORWARDED_HOST, HeaderValue::from_static(forwarded));
        let from = Origin::new(peer.parse().unwrap(), &headers, &TrustedProxies::default());
        same_origin(&headers, &from)
    }

    #[test]
    fn only_a_trusted_proxy_is_believed_about_the_host() {
        let page = "https://music.example";
        assert!(allowed("127.0.0.1", page, "jewelcase:8080", "music.example"));
        // The first of several forwarded hosts is the one the client asked for.
        assert!(allowed("127.0.0.1", page, "jewelcase:8080", "music.example, jewelcase:8080"));
        assert!(!allowed("203.0.113.9", page, "jewelcase:8080", "music.example"));
        assert!(!allowed("172.17.0.1", page, "jewelcase:8080", "music.example"));
        // `Host` is believed from anyone.
        assert!(allowed("203.0.113.9", page, "music.example", "jewelcase:8080"));
    }
}
