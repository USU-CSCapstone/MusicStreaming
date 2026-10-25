//! The addresses a plugin's requests may reach: never loopback, link-local, cloud metadata or
//! unspecified, and a private one only at a destination that names it, whether the URL names
//! the address or a name or a redirect leads there, or a kept reply came from it.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::Command;
use std::time::{Instant, SystemTime};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use super::super::super::Run;
use super::super::super::testing::{self, Memory};
use super::super::guard::Guard;
use super::super::http::{Error, Host as _};
use super::super::{Reach, build, rules};
use super::{get, run, server};
use crate::network::cache::{Key, Reply};

/// Answers its addresses for any name, in place of the system's resolver.
struct Fixed(Vec<IpAddr>);

impl Resolve for Fixed {
    /// Its addresses, whatever the name.
    fn resolve(&self, _: Name) -> Resolving {
        let found: Vec<SocketAddr> = self.0.iter().map(|ip| SocketAddr::new(*ip, 0)).collect();
        Box::pin(std::future::ready(Ok(Box::new(found.into_iter()) as Addrs)))
    }
}

/// What an address is to a request in these tests: what it is to any plugin's, except that the
/// test server's, 127.0.0.1, is public, so a test reaches its server and nothing else on
/// loopback.
fn only_the_server(ip: IpAddr) -> Reach {
    if ip == IpAddr::from(Ipv4Addr::LOCALHOST) { Reach::Public } else { rules::reach(ip) }
}

/// A run granted `destinations`, judging addresses by `reach`, whose client looks every name up
/// as `addresses`.
fn guarded(destinations: &[&str], reach: fn(IpAddr) -> Reach, addresses: &[IpAddr]) -> Run<Memory> {
    let mut run = run();
    run.grants.destinations = destinations.iter().map(|d| d.to_string()).collect();
    run.reach = reach;
    let client = build(&run.grants.destinations, reach, Fixed(addresses.to_vec())).unwrap();
    *run.client.lock().unwrap() = Some((run.grants.destinations.clone(), client));
    run
}

/// What the guard answers for a name whose addresses are `addresses`, with `destinations`.
async fn resolve(destinations: &[&str], addresses: &[IpAddr]) -> Result<Vec<SocketAddr>, String> {
    let destinations = destinations.iter().map(|d| d.to_string()).collect();
    let guard = Guard { lookup: Fixed(addresses.to_vec()), destinations, reach: rules::reach };
    let found = guard.resolve("nas.lan".parse().unwrap()).await;
    found.map(Iterator::collect).map_err(|e| e.to_string())
}

#[tokio::test]
async fn a_url_naming_loopback_is_refused_before_it_is_sent() {
    let (port, hits) = server().await;
    for (url, named) in [
        (format!("http://127.0.0.1:{port}/"), "127.0.0.1"),
        (format!("http://0.0.0.0:{port}/"), "0.0.0.0"),
    ] {
        let mut run = guarded(&[named], rules::reach, &[]);
        let refused = run.send(get(url)).await.unwrap_err();
        let why = format!("{named} is an address no plugin may reach");
        assert!(matches!(&refused, Error::Refused(r) if *r == why), "{refused:?}");
    }
    assert!(hits.lock().unwrap().is_empty(), "the server is never asked");
}

#[tokio::test]
async fn a_name_at_loopback_is_refused() {
    let (port, hits) = server().await;
    let mut run = guarded(&["loopback.test"], rules::reach, &[Ipv4Addr::LOCALHOST.into()]);
    for scheme in ["http", "https"] {
        let url = format!("{scheme}://loopback.test:{port}/");
        let refused = run.send(get(url)).await.unwrap_err();
        let why = "loopback.test: 127.0.0.1 is an address no plugin may reach";
        assert!(matches!(&refused, Error::Refused(r) if r == why), "{scheme}: {refused:?}");
    }
    assert_eq!(run.refused.named, ["loopback.test"], "recorded once");
    assert!(hits.lock().unwrap().is_empty(), "the server is never asked");
}

/// The client uses no proxy, even one the environment names: the test above, run again in a
/// process with a proxy set, still finds the name refused rather than sent on. The proxy's port
/// is closed, so a request sent there fails at once, and each is bounded by the run's timeouts
/// besides.
#[test]
fn a_proxy_in_the_environment_is_not_used() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let proxy = format!("http://{closed}");
    let test = "host::http::tests::addresses::a_name_at_loopback_is_refused";
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test])
        .envs([("HTTP_PROXY", &proxy), ("HTTPS_PROXY", &proxy)])
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .env_remove("REQUEST_METHOD")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&child.stdout);
    assert!(child.status.success() && out.contains("1 passed"), "{out}");
}

/// A redirect to loopback is refused, whether it names the address or a name that has it.
#[tokio::test]
async fn a_redirect_to_loopback_is_refused() {
    let (port, _) = server().await;
    let destinations = ["127.0.0.1", "localhost", "[::1]"];
    let mut run = guarded(&destinations, only_the_server, &[Ipv6Addr::LOCALHOST.into()]);
    let refused = run.send(get(format!("http://127.0.0.1:{port}/loop"))).await.unwrap_err();
    assert!(
        matches!(&refused, Error::Failed(why) if why.contains("redirected elsewhere: ::1 is an address no plugin may reach")),
        "{refused:?}"
    );
    let refused = run.send(get(format!("http://127.0.0.1:{port}/away"))).await.unwrap_err();
    let why = "localhost: ::1 is an address no plugin may reach";
    assert!(matches!(&refused, Error::Refused(r) if r == why), "{refused:?}");
    // Only the name the resolver refused is the plugin's to answer for.
    assert_eq!(run.refused.named, ["localhost"]);
}

#[tokio::test]
async fn a_named_host_reaches_its_private_address_and_any_destination_does_not() {
    let lan: IpAddr = Ipv4Addr::new(192, 168, 1, 10).into();
    assert_eq!(resolve(&["nas.lan"], &[lan]).await, Ok(vec![SocketAddr::new(lan, 0)]));
    let refused = resolve(&["*"], &[lan]).await.unwrap_err();
    assert!(refused.starts_with("nas.lan: 192.168.1.10 is a private address"), "{refused}");
}

/// A name's addresses out of reach are left out, and one with none in reach is refused.
#[tokio::test]
async fn a_name_reaches_only_its_addresses_in_reach() {
    let public: IpAddr = Ipv4Addr::new(1, 1, 1, 1).into();
    let metadata: IpAddr = Ipv4Addr::new(169, 254, 169, 254).into();
    let loopback: IpAddr = Ipv6Addr::LOCALHOST.into();
    let only_public = Ok(vec![SocketAddr::new(public, 0)]);
    assert_eq!(resolve(&["*"], &[metadata, public, loopback]).await, only_public);
    let refused = resolve(&["*"], &[metadata, loopback]).await.unwrap_err();
    assert_eq!(refused, "nas.lan: 169.254.169.254 is an address no plugin may reach");
}

/// A reply kept from a private address is not handed to a plugin that reaches its host only
/// through `*`: it is fetched again, and the guard refuses that. The test server stands for a
/// host on the LAN: loopback is private to tests (`testing::reach`).
#[tokio::test]
async fn a_reply_kept_from_a_private_address_is_not_reused_through_any_destination() {
    let (port, hits) = server().await;
    let at_the_server = [Ipv4Addr::LOCALHOST.into()];
    let mut named = guarded(&["nas.lan"], testing::reach, &at_the_server);
    let url = format!("http://nas.lan:{port}/fresh");
    assert_eq!(named.send(get(url.clone())).await.unwrap().body, b"1");
    let mut any = guarded(&["*"], testing::reach, &at_the_server);
    any.network = named.network.clone();
    let refused = any.send(get(url)).await.unwrap_err();
    let why = "nas.lan: 127.0.0.1 is a private address";
    assert!(matches!(&refused, Error::Refused(r) if r.starts_with(why)), "{refused:?}");
    assert_eq!(hits.lock().unwrap()["/fresh"], 1, "the fetch again never reaches the server");
}

/// Nor is a reply kept after a redirect to a private address that a plugin's destinations name.
#[tokio::test]
async fn a_reply_kept_from_a_redirect_to_a_private_address_is_not_reused_through_any_destination() {
    let (port, hits) = server().await;
    let at_the_server = [Ipv4Addr::LOCALHOST.into()];
    let mut named = guarded(&["a.test", "127.0.0.1"], testing::reach, &at_the_server);
    let url = format!("http://a.test:{port}/to-ip");
    assert_eq!(named.send(get(url.clone())).await.unwrap().body, b"1");
    let mut any = guarded(&["*"], testing::reach, &at_the_server);
    any.network = named.network.clone();
    let failed = any.send(get(url)).await.unwrap_err();
    let why = "redirected elsewhere: 127.0.0.1 is a private address";
    assert!(matches!(&failed, Error::Failed(r) if r.contains(why)), "{failed:?}");
    assert_eq!(hits.lock().unwrap()["/fresh"], 1, "nor is it fetched again");
}

/// A kept reply from an address no one knows is reused by no plugin, since none can be shown to
/// reach it: it is fetched again, and the guard judges that.
#[tokio::test]
async fn a_reply_from_an_unknown_address_is_fetched_again() {
    let (port, hits) = server().await;
    let mut run = guarded(&["*"], rules::reach, &[Ipv4Addr::LOCALHOST.into()]);
    let url = format!("http://public.test:{port}/");
    let key = Key::new(run.visible(), &url, &[]).unwrap();
    let headers = vec![("cache-control".into(), "max-age=60".into())];
    let reply =
        Reply { status: 200, headers, body: Default::default(), url: url.clone(), addr: None };
    run.network.cache.store(key, reply, Instant::now(), SystemTime::now());
    let refused = run.send(get(url)).await.unwrap_err();
    let why = "public.test: 127.0.0.1 is an address no plugin may reach";
    assert!(matches!(&refused, Error::Refused(r) if r == why), "{refused:?}");
    assert!(hits.lock().unwrap().is_empty(), "the server is never asked");
}
