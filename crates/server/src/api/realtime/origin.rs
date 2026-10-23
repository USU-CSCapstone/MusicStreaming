//! Whether a realtime connection comes from this server's own pages.

use axum::http::uri::Authority;
use axum::http::{HeaderMap, header};

/// Set by proxies that change `Host` (`requirements/deployment.md` §6).
const FORWARDED_HOST: &str = "x-forwarded-host";

/// Whether the request is not from a page, which sends no `Origin`, or is from a page this
/// server served: one whose origin names the host and port the request was sent to.
pub(super) fn same_origin(headers: &HeaderMap) -> bool {
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
    // A proxy that lists several hosts puts the one the client asked for first.
    let host = |name| {
        let host = headers.get(name)?.to_str().ok()?.split(',').next()?;
        host.trim().parse::<Authority>().ok()
    };
    [host(header::HOST.as_str()), host(FORWARDED_HOST)].into_iter().flatten().any(|host| {
        host.host().eq_ignore_ascii_case(origin.host()) && host.port_u16().or(default) == port
    })
}
