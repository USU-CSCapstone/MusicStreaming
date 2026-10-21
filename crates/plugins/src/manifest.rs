//! The manifest a plugin file carries (`design/plugins.md` §6): UTF-8 JSON in a top-level
//! custom section named `jewelcase:manifest` of a WebAssembly component.
//!
//! This is the only validator: `tools/plugin-pack` checks only that a manifest is a JSON object,
//! so an author learns what else is wrong at install. `validate` is tested against the cases in
//! `manifest-cases.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use wasmparser::{Encoding, Parser, Payload};

use crate::settings::Schema;

mod permission;
mod rate_limit;
#[cfg(test)]
mod tests;
mod validate;

pub use permission::Permission;
pub use rate_limit::{Limit, Per, RateLimit, rate_limit_gap};
pub use validate::validate;

pub const SECTION: &str = "jewelcase:manifest";

/// The version of the plugin contract (`wit/plugin.wit`) a manifest's `apiVersion` must give.
pub const API_VERSION: &str = "0.1";

/// The shortest interval a schedule may ask for.
pub const MIN_EVERY_MINUTES: u64 = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// The contract it was built against: [`API_VERSION`].
    pub api_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub permissions: Vec<PermissionRequest>,
    /// What an admin can set for it (`settings`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Schema>,
    /// What each user sets for themselves, such as their own account on a service.
    #[serde(default, rename = "personalSettings", skip_serializing_if = "Option::is_none")]
    pub personal_settings: Option<Schema>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub permission: Permission,
    pub required: bool,
    /// The author's reason, shown to the admin beside the request.
    pub reason: String,
    /// For network: host names, or `*` for any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destinations: Option<Vec<String>>,
    /// For network: the most often each destination may be asked, such as `"1/s"`
    /// ([`rate_limit_gap`]), and whose requests that counts.
    #[serde(default, rename = "rateLimits", skip_serializing_if = "Option::is_none")]
    pub rate_limits: Option<BTreeMap<String, Limit>>,
    /// For schedule: how often it runs.
    #[serde(default, rename = "everyMinutes", skip_serializing_if = "Option::is_none")]
    pub every_minutes: Option<u32>,
}

impl Manifest {
    /// The hosts its network request names; none if it asks for no network.
    pub fn destinations(&self) -> &[String] {
        let network = self.permissions.iter().find(|r| r.permission == Permission::Network);
        network.and_then(|r| r.destinations.as_deref()).unwrap_or_default()
    }

    /// The rate limit its network request declares for each host that has one, the host
    /// lowercase.
    pub fn rate_limits(&self) -> Vec<RateLimit> {
        let network = self.permissions.iter().find(|r| r.permission == Permission::Network);
        let limits = network.and_then(|r| r.rate_limits.as_ref()).into_iter().flatten();
        let limit = |(host, limit): (&String, &Limit)| {
            let (pace, per) = match limit {
                Limit::Pace(pace) => (pace, Per::Plugin),
                Limit::Counted { limit, per } => (limit, *per),
            };
            let gap = rate_limit_gap(pace)?;
            Some(RateLimit { host: host.to_ascii_lowercase(), limit: pace.clone(), gap, per })
        };
        limits.filter_map(limit).collect()
    }

    /// How often its schedule runs, if it asks for one.
    pub fn every_minutes(&self) -> Option<u32> {
        let schedule = self.permissions.iter().find(|r| r.permission == Permission::Schedule);
        schedule.and_then(|r| r.every_minutes)
    }

    pub fn required(&self) -> impl Iterator<Item = Permission> + '_ {
        self.permissions.iter().filter(|r| r.required).map(|r| r.permission)
    }
}

/// Why a file is not a plugin, in words an admin can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidPlugin(pub String);

/// The manifest `bytes` carries, validated.
pub fn read(bytes: &[u8]) -> Result<Manifest, InvalidPlugin> {
    let invalid = |message: &str| InvalidPlugin(message.to_owned());
    let mut found = Vec::new();
    // Nested modules and components carry custom sections of their own; only the top level's
    // count, so the walk tracks how deep it is.
    let mut depth = 0_u32;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|_| invalid("This is not a WebAssembly file."))?;
        match payload {
            Payload::Version { encoding: Encoding::Module, .. } if depth == 0 => {
                return Err(invalid(
                    "This is a WebAssembly module, not a component. Plugins are built as components.",
                ));
            }
            Payload::ModuleSection { .. } | Payload::ComponentSection { .. } => depth += 1,
            Payload::End(_) => depth = depth.saturating_sub(1),
            Payload::CustomSection(section) if depth == 0 && section.name() == SECTION => {
                found.push(section.data());
            }
            _ => {}
        }
    }
    let data = match found.as_slice() {
        [] => {
            return Err(invalid(
                "This component has no Jewelcase manifest. Pack it with tools/plugin-pack first.",
            ));
        }
        [data] => *data,
        _ => return Err(invalid("This component has more than one manifest.")),
    };
    let value: Value =
        serde_json::from_slice(data).map_err(|_| invalid("The manifest is not valid JSON."))?;
    let problems = validate(&value);
    if !problems.is_empty() {
        return Err(InvalidPlugin(format!("The manifest is invalid: {}.", problems.join("; "))));
    }
    serde_json::from_value(value).map_err(|_| invalid("The manifest is not valid JSON."))
}

/// A problem for each key of `o` that is not one of `known`, found at `at` in the manifest
/// (empty for its top level).
pub(crate) fn unknown_keys(at: &str, o: &Map<String, Value>, known: &[&str]) -> Vec<String> {
    let at = if at.is_empty() { String::new() } else { format!("{at}: ") };
    let unknown = o.keys().filter(|key| !known.contains(&key.as_str()));
    unknown.map(|key| format!(r#"{at}unknown key "{key}""#)).collect()
}

/// Whether `host` is one host, as a destination, a rate limit, or an exemption from one names
/// it: one the URL parser gives back unchanged but for ASCII case, since a request's parsed host
/// is matched against it that way (`rules::allowed`). So a pattern, a URL, a port, a trailing
/// dot, an IPv4 shorthand such as `127.1`, IPv6 outside brackets, or an IDN not in punycode
/// never is.
pub fn is_host_name(host: &str) -> bool {
    let parsed = reqwest::Url::parse(&format!("http://{host}/")).ok();
    let same =
        parsed.as_ref().and_then(|u| u.host_str()).is_some_and(|h| h.eq_ignore_ascii_case(host));
    same && !host.contains('*') && !host.ends_with('.')
}
