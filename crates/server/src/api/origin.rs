//! Where a request comes from, for the per-origin sign-in limits (`requirements/users.md`
//! §3.1), including behind a reverse proxy (`requirements/deployment.md` §7).
//!
//! A peer on a private network or this host may be a proxy, so its `X-Forwarded-For` is
//! believed: the client is the nearest address in it that is not private. That lets anyone on
//! the private network claim any origin, and the per-account limits still hold them.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::HeaderMap;
use axum::http::request::Parts;

use super::{Code, Problem};

const FORWARDED_FOR: &str = "x-forwarded-for";

/// The client's address.
pub struct Origin(pub IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for Origin {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Origin, Problem> {
        // `run` serves with connection info, and tests add it, so it is always there.
        let peer = ConnectInfo::<SocketAddr>::from_request_parts(parts, state)
            .await
            .map_err(|_| Problem::new(Code::Internal))?;
        Ok(Origin(client(peer.ip(), &parts.headers)))
    }
}

/// Walks back through the proxies that forwarded the request, from the peer, and stops at the
/// first address that is not private, or at anything that is not an address.
fn client(peer: IpAddr, headers: &HeaderMap) -> IpAddr {
    let mut client = peer.to_canonical();
    if !private(client) {
        return client;
    }
    let values = headers.get_all(FORWARDED_FOR).iter().filter_map(|value| value.to_str().ok());
    // Each proxy appends the address it heard from, so the nearest comes last.
    for hop in values.flat_map(|value| value.split(',')).rev() {
        let Ok(hop) = hop.trim().parse::<IpAddr>() else { break };
        client = hop.to_canonical();
        if !private(client) {
            break;
        }
    }
    client
}

fn private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from(peer: &str, forwarded: &[&str]) -> String {
        let mut headers = HeaderMap::new();
        for value in forwarded {
            headers.append(FORWARDED_FOR, value.parse().unwrap());
        }
        client(peer.parse().unwrap(), &headers).to_string()
    }

    #[test]
    fn a_public_peer_is_the_client_whatever_it_claims() {
        assert_eq!(from("203.0.113.9", &[]), "203.0.113.9");
        assert_eq!(from("203.0.113.9", &["198.51.100.1"]), "203.0.113.9");
        assert_eq!(from("2001:db8::9", &["198.51.100.1"]), "2001:db8::9");
    }

    #[test]
    fn behind_private_proxies_the_client_is_the_nearest_public_address() {
        assert_eq!(from("127.0.0.1", &["198.51.100.1"]), "198.51.100.1");
        assert_eq!(from("::ffff:172.18.0.2", &["198.51.100.1"]), "198.51.100.1");
        // Forged on the left by the client, appended truthfully on the right by the proxies.
        assert_eq!(from("10.0.0.2", &["192.0.2.66, 198.51.100.1, 10.0.0.3"]), "198.51.100.1");
        assert_eq!(from("10.0.0.2", &["192.0.2.66", "198.51.100.1"]), "198.51.100.1");
        assert_eq!(from("fd00::2", &["2001:db8::1"]), "2001:db8::1");
    }

    #[test]
    fn a_client_on_the_private_network_is_its_own_address() {
        assert_eq!(from("192.168.1.20", &[]), "192.168.1.20");
        assert_eq!(from("10.0.0.2", &["192.168.1.20"]), "192.168.1.20");
    }

    #[test]
    fn the_walk_stops_at_anything_that_is_not_an_address() {
        assert_eq!(from("10.0.0.2", &["198.51.100.1, unknown"]), "10.0.0.2");
        assert_eq!(from("10.0.0.2", &["198.51.100.1, 192.168.1.20:4000"]), "10.0.0.2");
    }
}
