//! What the host's tests share: a plugin's grants, and a run over a library built without a
//! plugin, so a test calls the host imports directly. The library in memory they read is the
//! crate's [`testing`](crate::testing) one.

use std::net::IpAddr;
use std::sync::Arc;

use wasmtime::StoreLimitsBuilder;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::WasiCtx;

use super::{Grants, Library, Run};
use crate::manifest::Permission;
use crate::rules::{self, Reach};
pub use crate::testing::{Memory, album, artist, track};

/// The grants of plugin `test` in library 1: `permissions`, with 127.0.0.1 its only
/// destination, for no user, with no rate limits or settings.
pub fn grants(permissions: &[Permission]) -> Grants {
    Grants::new("test", 1, permissions, &["127.0.0.1"])
}

/// What an address is to a test's requests: what it is to any plugin's, except that loopback,
/// where the tests' servers listen, is private, so a test that names its server reaches it.
pub fn reach(ip: IpAddr) -> Reach {
    if ip.to_canonical().is_loopback() { Reach::Private } else { rules::reach(ip) }
}

/// A run over `library` with `grants(permissions)`, and a network and client of its own. Its
/// requests reach loopback as `reach` says.
pub fn run<L: Library>(library: L, permissions: &[Permission]) -> Run<L> {
    Run {
        wasi: WasiCtx::builder().build(),
        table: ResourceTable::new(),
        limits: StoreLimitsBuilder::new().build(),
        ticks: 0,
        grants: grants(permissions),
        library,
        client: Arc::default(),
        network: Arc::default(),
        roots: None,
        log: Vec::new(),
        written: 0,
        touched: Vec::new(),
        refused: Default::default(),
        timeout: super::http::TIMEOUT,
        reach,
    }
}
