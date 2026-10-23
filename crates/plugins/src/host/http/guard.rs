//! The resolver of a plugin's HTTP client, which keeps its connections to the addresses it may
//! reach (`rules::may_reach`). A URL that names an address skips it, so `rules::allowed` checks
//! those.

use std::net::IpAddr;
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::rules::{self, Reach};

/// Looks names up with `lookup`, and answers only the addresses that are in reach, refusing a
/// name with none. The client connects only to an address its resolver answered, so judging
/// them here judges every connection: to a destination, after a redirect, and when a name's
/// addresses change between a check and the connection, which a check before sending would miss.
pub(super) struct Guard<R> {
    pub lookup: R,
    pub destinations: Arc<[String]>,
    pub reach: fn(IpAddr) -> Reach,
}

/// A name refused because none of its addresses is in reach, and why. The plugin sees it as
/// `refused`.
#[derive(Debug, thiserror::Error)]
#[error("{host}: {why}")]
pub(super) struct Forbidden {
    /// The name refused.
    pub host: String,
    /// Why its addresses are out of reach.
    why: String,
}

impl<R: Resolve> Resolve for Guard<R> {
    /// The addresses `lookup` finds for `name` that are in reach, or [`Forbidden`] if it finds
    /// some and none is.
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        let found = self.lookup.resolve(name);
        let (destinations, reach) = (self.destinations.clone(), self.reach);
        Box::pin(async move {
            let (mut allowed, mut refused) = (Vec::new(), None);
            for address in found.await? {
                match rules::may_reach(&host, address.ip(), &destinations, reach) {
                    Ok(()) => allowed.push(address),
                    Err(why) => refused = refused.or(Some(why)),
                }
            }
            match refused {
                Some(why) if allowed.is_empty() => Err(Forbidden { host, why }.into()),
                _ => Ok(Box::new(allowed.into_iter()) as Addrs),
            }
        })
    }
}

/// Looks a name up with the system's resolver on a blocking thread, as reqwest's own does,
/// which reqwest does not export for `Guard` to wrap.
pub(super) struct System;

impl Resolve for System {
    /// The addresses the system finds for `name`.
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let found = tokio::net::lookup_host((host, 0)).await?;
            Ok(Box::new(found) as Addrs)
        })
    }
}

/// The name the resolver refused, and why, if that is why `error` failed to connect.
pub(super) fn forbidden(error: &reqwest::Error) -> Option<&Forbidden> {
    let mut causes = std::iter::successors(std::error::Error::source(error), |e| e.source());
    causes.find_map(|cause| cause.downcast_ref::<Forbidden>())
}
