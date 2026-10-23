//! Where a request comes from: the client's address, for the per-origin sign-in limits
//! (`requirements/users.md` §3.1), and whether it reached the server over https, including
//! behind a reverse proxy (`requirements/deployment.md` §6).
//!
//! Only a peer that `JEWELCASE_TRUSTED_PROXIES` names is believed about what it forwards, and
//! every `X-Forwarded-*` header is read through [`Origin::forwarded`]. Behind trusted proxies, the
//! client is the nearest address in `X-Forwarded-For` that is not one; any other peer is the
//! client, whatever it claims. At an origin an account has signed in from, that account's limit
//! counts only the origin's failures (`login::limits`), so whoever shares one, such as everyone
//! behind the owner's carrier-grade NAT or on their campus network, or claims it from a trusted
//! proxy's address, still gets only the account's few free guesses at its password there. Once a
//! name is locked out elsewhere, they can also tell there whether it is an active account that
//! has signed in from that origin: its password is still checked (401), where any other name
//! waits (429).

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::HeaderMap;
use axum::http::request::Parts;

use super::{AppState, Code, Problem};
use crate::config::TrustedProxies;

const FORWARDED_FOR: &str = "x-forwarded-for";
const FORWARDED_PROTO: &str = "x-forwarded-proto";

/// Where a request comes from, as its peer and the proxies the server trusts tell it.
pub struct Origin {
    /// The client's address.
    pub client: IpAddr,
    /// Whether the client reached the server over https, which only a trusted proxy can say.
    pub https: bool,
    /// Whether the peer is a trusted proxy.
    proxied: bool,
}

impl FromRequestParts<AppState> for Origin {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Origin, Problem> {
        // `serve` adds connection info to every request, and tests add it, so it is always there.
        let peer = ConnectInfo::<SocketAddr>::from_request_parts(parts, state)
            .await
            .map_err(|_| Problem::new(Code::Internal))?;
        Ok(Origin::new(peer.ip(), &parts.headers, &state.trusted_proxies))
    }
}

impl Origin {
    /// Where a request from `peer` with `headers` comes from. The walk back through the proxies
    /// that forwarded it stops at the first address that is not a trusted proxy, or at anything
    /// that is not an address.
    pub(super) fn new(peer: IpAddr, headers: &HeaderMap, trusted: &TrustedProxies) -> Origin {
        let peer = peer.to_canonical();
        let mut origin = Origin { client: peer, https: false, proxied: trusted.trusts(peer) };
        let proto = origin.forwarded(headers, FORWARDED_PROTO).next();
        origin.https = proto.is_some_and(|proto| proto.eq_ignore_ascii_case("https"));
        let mut client = peer;
        // Each proxy appends the address it heard from, so the nearest comes last.
        for hop in origin.forwarded(headers, FORWARDED_FOR).rev() {
            let Ok(hop) = hop.parse::<IpAddr>() else { break };
            client = hop.to_canonical();
            if !trusted.trusts(client) {
                break;
            }
        }
        origin.client = client;
        origin
    }

    /// The comma-separated entries of the forwarded header `name`, trimmed, in the order they
    /// were sent, if the peer is a trusted proxy, and none otherwise. A line that is not
    /// visible ASCII is one empty entry.
    pub(super) fn forwarded<'a>(
        &self,
        headers: &'a HeaderMap,
        name: &str,
    ) -> impl DoubleEndedIterator<Item = &'a str> {
        let values = self.proxied.then(|| headers.get_all(name)).into_iter().flatten();
        // A line that is not text is an empty entry rather than none, so a walk stops there
        // rather than skipping it.
        values
            .map(|value| value.to_str().unwrap_or(""))
            .flat_map(|value| value.split(',').map(str::trim))
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    /// The origin of a request from `peer` with `headers`, behind the default trusted proxies.
    fn origin(peer: &str, headers: &[(&'static str, &str)]) -> Origin {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(*name, value.parse().unwrap());
        }
        Origin::new(peer.parse().unwrap(), &map, &TrustedProxies::default())
    }

    /// The client of a request from `peer` with `forwarded` as its `X-Forwarded-For` lines.
    /// The client a request from `peer` comes from, given one `X-Forwarded-For` header per entry
    /// of `forwarded`.
    fn from(peer: &str, forwarded: &[&str]) -> String {
        let headers: Vec<_> = forwarded.iter().map(|value| (FORWARDED_FOR, *value)).collect();
        origin(peer, &headers).client.to_string()
    }

    #[test]
    fn an_untrusted_peer_is_the_client_whatever_it_claims() {
        assert_eq!(from("203.0.113.9", &[]), "203.0.113.9");
        assert_eq!(from("203.0.113.9", &["198.51.100.1"]), "203.0.113.9");
        assert_eq!(from("2001:db8::9", &["198.51.100.1"]), "2001:db8::9");
        assert_eq!(from("10.0.0.2", &["198.51.100.1"]), "10.0.0.2");
        assert_eq!(from("::ffff:203.0.113.9", &[]), "203.0.113.9");
        assert_eq!(from("172.17.0.1", &["198.51.100.1"]), "172.17.0.1");
    }

    #[test]
    fn behind_trusted_proxies_the_client_is_the_nearest_untrusted_address() {
        assert_eq!(from("127.0.0.1", &["198.51.100.1"]), "198.51.100.1");
        assert_eq!(from("::ffff:127.0.0.1", &["198.51.100.1"]), "198.51.100.1");
        assert_eq!(from("::1", &["2001:db8::1"]), "2001:db8::1");
        assert_eq!(from("127.0.0.1", &["::ffff:198.51.100.1"]), "198.51.100.1");
        // Forged on the left by the client, appended truthfully on the right by the proxies.
        assert_eq!(from("127.0.0.1", &["192.0.2.66, 198.51.100.1, 127.0.0.3"]), "198.51.100.1");
        assert_eq!(from("127.0.0.1", &["192.0.2.66", "198.51.100.1"]), "198.51.100.1");
        // A private address is a client like any other.
        assert_eq!(from("127.0.0.1", &["192.0.2.66, 10.0.0.3"]), "10.0.0.3");
    }

    #[test]
    fn the_walk_stops_at_anything_that_is_not_an_address() {
        assert_eq!(from("127.0.0.1", &["198.51.100.1, unknown"]), "127.0.0.1");
        assert_eq!(from("127.0.0.1", &["198.51.100.1, 192.168.1.20:4000"]), "127.0.0.1");
    }

    #[test]
    fn the_walk_stops_at_a_line_that_is_not_text() {
        let mut headers = HeaderMap::new();
        headers.append(FORWARDED_FOR, HeaderValue::from_static("198.51.100.77"));
        headers.append(FORWARDED_FOR, HeaderValue::from_bytes(b"\xff, 127.0.0.2").unwrap());
        let origin = Origin::new("127.0.0.1".parse().unwrap(), &headers, &Default::default());
        // Skipping the line would make the forged address on the line before it the client.
        assert_eq!(origin.client.to_string(), "127.0.0.1");
    }

    #[test]
    fn only_a_trusted_proxy_says_the_client_used_https() {
        assert!(origin("127.0.0.1", &[(FORWARDED_PROTO, "https")]).https);
        assert!(origin("127.0.0.1", &[(FORWARDED_PROTO, "HTTPS, http")]).https);
        assert!(!origin("127.0.0.1", &[(FORWARDED_PROTO, "http")]).https);
        assert!(!origin("127.0.0.1", &[]).https);
        assert!(!origin("203.0.113.9", &[(FORWARDED_PROTO, "https")]).https);
        assert!(!origin("10.0.0.2", &[(FORWARDED_PROTO, "https")]).https);
    }
}
