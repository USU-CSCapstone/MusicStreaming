//! Which destinations and addresses a plugin may reach over the network, kept apart from
//! Wasmtime so it is testable.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Alibaba Cloud's metadata service, outside the link-local range.
const METADATA_V4: Ipv4Addr = Ipv4Addr::new(100, 100, 100, 200);
/// AWS's metadata service over IPv6, outside the link-local range.
const METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254);

/// What an address is to a plugin's requests (`requirements/plugins.md` §4.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reach {
    /// Reached at any destination the plugin may reach, `*` included: the services plugins
    /// exist to use.
    Public,
    /// On a private network: reached only at a destination that names the host outright.
    /// Whoever controls a name decides where it leads, so through `*` any name could lead a
    /// plugin onto the LAN; a destination that names the host is the admin trusting its owner.
    Private,
    /// Never reached, whatever the destinations: through these a plugin could reach Jewelcase
    /// itself, or another service on the server, as a trusted local client, or read the
    /// host's cloud credentials.
    Never,
}

/// What `ip` is to a plugin's requests, with an IPv4 address mapped into IPv6 taken as itself.
/// Loopback, link-local, cloud metadata, and unspecified addresses, 0.0.0.0/8 among them, are
/// never reached. Private ranges, shared address space (100.64.0.0/10, as Tailscale uses), and
/// IPv6 unique local addresses are private. Everything else is public, the server's own other
/// addresses and global IPv6 ones on a LAN among them.
pub fn reach(ip: IpAddr) -> Reach {
    match ip.to_canonical() {
        IpAddr::V4(ip) => {
            let [first, second, ..] = ip.octets();
            if ip.is_loopback() || ip.is_link_local() || first == 0 || ip == METADATA_V4 {
                Reach::Never
            } else if ip.is_private() || (first == 100 && second & 0xc0 == 64) {
                Reach::Private
            } else {
                Reach::Public
            }
        }
        IpAddr::V6(ip) => {
            if ip.is_loopback()
                || ip.is_unicast_link_local()
                || ip.is_unspecified()
                || ip == METADATA_V6
            {
                Reach::Never
            } else if ip.is_unique_local() {
                Reach::Private
            } else {
                Reach::Public
            }
        }
    }
}

/// Whether a request to `host`, at `ip`, may go there, judged by `reach`: never to an address
/// no plugin reaches, and to a private one only if `destinations` name `host` rather than
/// reach it through `*`.
pub fn may_reach(
    host: &str,
    ip: IpAddr,
    destinations: &[String],
    reach: fn(IpAddr) -> Reach,
) -> Result<(), String> {
    match reach(ip) {
        Reach::Public => Ok(()),
        Reach::Private if names(host, destinations) => Ok(()),
        Reach::Private => Err(format!(
            "{ip} is a private address, which only a destination that names its host reaches"
        )),
        Reach::Never => Err(format!("{ip} is an address no plugin may reach")),
    }
}

/// Whether `url` may be fetched, given the destinations the manifest declared and what each
/// address is to the plugin. Only http(s), and only to a listed host (or any, for `*`);
/// subdomains must be listed too. A host written as an address, in any form the URL parser
/// reads as one (`2130706433`, `0x7f.1`), must be one `may_reach` allows, since the client
/// connects to it without looking anything up; a name is checked as it is looked up instead
/// (`host/http/guard.rs`). Answers the URL as parsed, so every spelling of it is one, with its
/// host lowercase.
pub fn allowed(
    url: &str,
    destinations: &[String],
    reach: fn(IpAddr) -> Reach,
) -> Result<reqwest::Url, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "not a URL".to_owned())?;
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err(format!("{} is not allowed; only http and https", parsed.scheme()));
    }
    // Parsed, so userinfo and ports are already apart from the host, which is lowercase.
    let host = parsed.host_str().ok_or("the URL has no host")?;
    // `nas.` and `nas` can be different hosts, and `a.com.` would escape `a.com`'s rate limits,
    // so a host is named without the dot.
    if host.ends_with('.') {
        return Err(format!("{host} ends with a dot; name the host without it"));
    }
    if !reaches(host, destinations) {
        return Err(format!("{host} is not one of this plugin's approved destinations"));
    }
    // The parser writes an IPv6 address in brackets, and an IPv4 one in dotted decimal.
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    if let Ok(ip) = bare.parse() {
        may_reach(host, ip, destinations, reach)?;
    }
    Ok(parsed)
}

/// Whether `destinations` let a plugin reach `host`: one names it, or is `*`.
pub fn reaches(host: &str, destinations: &[String]) -> bool {
    destinations.iter().any(|d| d == "*") || names(host, destinations)
}

/// Whether one of `destinations` names `host` outright.
fn names(host: &str, destinations: &[String]) -> bool {
    destinations.iter().any(|d| d.eq_ignore_ascii_case(host))
}

/// The host `url` names, lowercase as the parser gives an http(s) host, or nothing if it is
/// not a URL.
pub fn host_of(url: &str) -> String {
    let url = reqwest::Url::parse(url).ok();
    url.as_ref().and_then(|url| url.host_str()).unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests;
