//! `JEWELCASE_TRUSTED_PROXIES`: the peers whose `X-Forwarded-*` headers the server believes
//! (`requirements/deployment.md` §6).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The proxies the server trusts, as addresses and networks. Loopback alone unless set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProxies(Vec<Network>);

/// An address, and how many of its leading bits a peer shares with it to be in the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Network {
    address: IpAddr,
    bits: u32,
}

impl Default for TrustedProxies {
    /// Loopback: `127.0.0.0/8` and `::1`.
    fn default() -> Self {
        TrustedProxies(vec![
            Network { address: Ipv4Addr::LOCALHOST.into(), bits: 8 },
            Network { address: Ipv6Addr::LOCALHOST.into(), bits: 128 },
        ])
    }
}

impl TrustedProxies {
    /// Whether `peer` is one of them. An IPv4 address mapped into IPv6 counts as itself.
    pub fn trusts(&self, peer: IpAddr) -> bool {
        let peer = peer.to_canonical();
        self.0.iter().any(|network| {
            // Where the two first differ must come after the bits the network fixes.
            let first_difference = match (network.address, peer) {
                (IpAddr::V4(a), IpAddr::V4(b)) => (a.to_bits() ^ b.to_bits()).leading_zeros(),
                (IpAddr::V6(a), IpAddr::V6(b)) => (a.to_bits() ^ b.to_bits()).leading_zeros(),
                _ => return false,
            };
            first_difference >= network.bits
        })
    }
}

/// Reads a comma-separated list of addresses and networks, such as `172.16.0.0/12, ::1`.
pub(super) fn parse(value: &str) -> Result<TrustedProxies, &'static str> {
    let networks: Option<Vec<_>> = value.split(',').map(|entry| network(entry.trim())).collect();
    networks
        .map(TrustedProxies)
        .ok_or("addresses or networks separated by commas, such as 127.0.0.1,172.16.0.0/12,::1")
}

/// Reads one entry: an address, which is a network of itself alone, or a network in CIDR
/// notation. An IPv4 address mapped into IPv6 is refused.
fn network(entry: &str) -> Option<Network> {
    let (address, bits) = match entry.split_once('/') {
        Some((address, bits)) => (address, Some(bits)),
        None => (entry, None),
    };
    let address: IpAddr = address.parse().ok()?;
    // `trusts` compares a mapped peer as IPv4, so a mapped entry would never match.
    if address.to_canonical() != address {
        return None;
    }
    let width = if address.is_ipv4() { 32 } else { 128 };
    let bits = match bits {
        Some(bits) => bits.parse().ok().filter(|bits| *bits <= width)?,
        None => width,
    };
    Some(Network { address, bits })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether `proxies` trusts each of `peers`.
    /// Whether `proxies` trusts each of `peers`.
    fn trusted(proxies: &TrustedProxies, peers: &[&str]) -> Vec<bool> {
        peers.iter().map(|peer| proxies.trusts(peer.parse().unwrap())).collect()
    }

    #[test]
    fn only_loopback_is_trusted_by_default() {
        let peers = ["127.0.0.1", "127.9.8.7", "::1", "::ffff:127.0.0.1"];
        assert_eq!(trusted(&TrustedProxies::default(), &peers), [true; 4]);
        let peers = ["10.0.0.1", "172.17.0.1", "192.168.1.2", "128.0.0.1", "::2", "fd00::1"];
        assert_eq!(trusted(&TrustedProxies::default(), &peers), [false; 6]);
    }

    #[test]
    fn a_network_trusts_the_addresses_in_it() {
        let proxies = parse("172.16.0.0/12, 2001:db8::/32,192.0.2.7").unwrap();
        let inside = ["172.16.0.1", "172.31.255.255", "::ffff:172.20.0.2", "2001:db8:1::5"];
        assert_eq!(trusted(&proxies, &inside), [true; 4]);
        assert_eq!(trusted(&proxies, &["192.0.2.7"]), [true]);
        let outside = ["172.15.255.255", "172.32.0.0", "2001:db9::", "192.0.2.8", "127.0.0.1"];
        assert_eq!(trusted(&proxies, &outside), [false; 5]);
    }

    #[test]
    fn a_full_length_network_is_its_address_alone() {
        let proxies = parse("192.0.2.7/32, ::1/128").unwrap();
        assert_eq!(trusted(&proxies, &["192.0.2.7", "::1"]), [true; 2]);
        let others = ["192.0.2.6", "192.0.2.8", "::", "::2", "127.0.0.1"];
        assert_eq!(trusted(&proxies, &others), [false; 5]);
    }

    #[test]
    fn a_network_of_no_fixed_bits_is_its_whole_family() {
        let proxies = parse("0.0.0.0/0").unwrap();
        assert_eq!(
            trusted(&proxies, &["203.0.113.9", "10.0.0.1", "2001:db8::1"]),
            [true, true, false]
        );
        let proxies = parse("::/0").unwrap();
        assert_eq!(trusted(&proxies, &["2001:db8::1", "203.0.113.9"]), [true, false]);
    }

    #[test]
    fn anything_but_addresses_and_networks_is_refused() {
        let refused = [
            "",
            " ",
            "localhost",
            "10.0.0.1,",
            "10.0.0.0/33",
            "::/129",
            "10.0.0.0/",
            "10.0.0.0/x",
            "10.0.0.1:80",
            "10.0.0.1 10.0.0.2",
            "::ffff:10.0.0.1",
            "::ffff:10.0.0.0/104",
        ];
        for value in refused {
            assert!(parse(value).is_err(), "{value:?}");
        }
    }
}
