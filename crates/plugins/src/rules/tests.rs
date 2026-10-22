use super::*;

/// `list` as owned strings, as a manifest's destinations are.
fn d(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Whether `url` may be fetched with `destinations`, as any plugin's request is judged.
fn ok(url: &str, destinations: &[&str]) -> bool {
    allowed(url, &d(destinations), reach).is_ok()
}

#[test]
fn destinations_are_exact() {
    let only = d(&["lrclib.net"]);
    assert!(allowed("https://lrclib.net/api/get?x=1", &only, reach).is_ok());
    let parsed = allowed("https://LRCLIB.net:443/api", &only, reach).map(String::from);
    assert_eq!(parsed, Ok("https://lrclib.net/api".into()));
    // A trailing dot is refused rather than read as the same host.
    assert!(!ok("https://lrclib.net./api", &["lrclib.net"]));
    assert!(!ok("https://lrclib.net./api", &["*"]));
    assert!(!ok("https://evil.example/api", &["lrclib.net"]));
    assert!(!ok("https://lrclib.net.evil.example/", &["lrclib.net"]));
    assert!(!ok("https://api.lrclib.net/", &["lrclib.net"]));
    assert!(!ok("https://lrclib.net@evil.example/", &["lrclib.net"]));
    assert!(!ok("file:///etc/passwd", &["lrclib.net"]));
    assert!(ok("https://anything.example/", &["*"]));
}

#[test]
fn loopback_link_local_metadata_and_unspecified_are_never_reached() {
    for ip in [
        "127.0.0.1",
        "127.255.255.254",
        "::1",
        "::ffff:127.0.0.1",
        "169.254.169.254",
        "fe80::1",
        "febf::1",
        "::ffff:169.254.169.254",
        "100.100.100.200",
        "fd00:ec2::254",
        "0.0.0.0",
        "0.1.2.3",
        "::",
        "::ffff:0.0.0.0",
    ] {
        assert_eq!(reach(ip.parse().unwrap()), Reach::Never, "{ip}");
    }
}

#[test]
fn private_networks_are_private() {
    for ip in [
        "10.0.0.1",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.10",
        "100.64.0.1",
        "100.127.255.255",
        "100.100.100.201",
        "fc00::1",
        "fd00:ec2::255",
        "fd7a:115c:a1e0::1",
        "::ffff:192.168.1.10",
    ] {
        assert_eq!(reach(ip.parse().unwrap()), Reach::Private, "{ip}");
    }
}

#[test]
fn everything_else_is_public() {
    for ip in [
        "1.1.1.1",
        "9.255.255.255",
        "172.32.0.1",
        "100.63.255.255",
        "100.128.0.1",
        "169.253.0.1",
        "2606:4700::1111",
        "fec0::1",
        "::ffff:1.1.1.1",
    ] {
        assert_eq!(reach(ip.parse().unwrap()), Reach::Public, "{ip}");
    }
}

/// An address in the URL is never one no plugin reaches, however it is written, and even when
/// a destination names it.
#[test]
fn a_url_never_names_an_address_out_of_reach() {
    for (url, named) in [
        ("http://127.0.0.1:8080/", "127.0.0.1"),
        ("http://[::ffff:127.0.0.1]/", "[::ffff:7f00:1]"),
        ("http://2130706433/", "127.0.0.1"),
        ("http://0177.0.0.1/", "127.0.0.1"),
        ("http://0x7f.1/", "127.0.0.1"),
        ("http://[::1]/", "[::1]"),
        ("http://169.254.169.254/latest/meta-data/", "169.254.169.254"),
        ("http://[fe80::1]/", "[fe80::1]"),
        ("http://0.0.0.0/", "0.0.0.0"),
        ("http://0/", "0.0.0.0"),
        ("http://[::]/", "[::]"),
    ] {
        assert!(!ok(url, &["*"]), "{url} through *");
        let refused = allowed(url, &d(&[named]), reach).unwrap_err();
        assert!(refused.ends_with("is an address no plugin may reach"), "{url}: {refused}");
    }
}

#[test]
fn a_private_address_needs_a_destination_that_names_it() {
    assert!(!ok("http://192.168.1.10/", &["*"]));
    assert!(ok("http://192.168.1.10:8686/api", &["192.168.1.10"]));
    assert!(ok("http://192.168.1.10/", &["*", "192.168.1.10"]));
    assert!(!ok("http://192.168.1.11/", &["*", "192.168.1.10"]));
    assert!(!ok("http://[fd00::5]/", &["*"]));
    assert!(ok("http://[fd00::5]/", &["[fd00::5]"]));
    assert!(ok("http://8.8.8.8/", &["*"]));
}

/// The URL parser takes no zone in an IPv6 address, so none reaches the connection.
#[test]
fn an_ipv6_zone_is_not_a_url() {
    assert_eq!(allowed("http://[fe80::1%25eth0]/", &d(&["*"]), reach), Err("not a URL".into()));
}

/// A name is judged by each address it has: a private one only if the destinations name it.
#[test]
fn a_named_host_reaches_its_private_address() {
    let lan: IpAddr = "192.168.1.10".parse().unwrap();
    assert!(may_reach("nas.lan", lan, &d(&["nas.lan"]), reach).is_ok());
    assert!(may_reach("nas.lan", lan, &d(&["*"]), reach).is_err());
    let loopback: IpAddr = "127.0.0.1".parse().unwrap();
    assert!(may_reach("localhost", loopback, &d(&["localhost"]), reach).is_err());
}
