//! The manifest a plugin file carries (`design/plugins.md` §6): UTF-8 JSON in a top-level
//! custom section named `jewelcase:manifest` of a WebAssembly component.
//!
//! This is the authority on what installs. `tools/plugin-pack` validates the same way for
//! authors, and both are held to `manifest-cases.json`, so they cannot drift apart.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use wasmparser::{Encoding, Parser, Payload};

use crate::settings::{self, Schema};

pub const SECTION: &str = "jewelcase:manifest";

/// The version of the plugin contract (`wit/plugin.wit`) a manifest's `apiVersion` must give.
pub const API_VERSION: &str = "0.1";

/// The shortest interval a schedule may ask for.
pub const MIN_EVERY_MINUTES: u64 = 5;

/// What a plugin can ask for (`requirements/plugins.md` §4.1): what it may reach, and the hooks
/// that run it, which are approved the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    LibraryRead,
    /// Creating files that do not exist yet.
    LibraryAdd,
    /// Replacing, renaming, and deleting files that do.
    LibraryChange,
    Network,
    ListeningActivity,
    /// A hook: run when tracks are added, changed, or removed. Needs library-read too.
    TracksChanged,
    /// A hook: run when a scan of the library finishes. Needs library-read too.
    ScanFinished,
    /// A hook: run at the interval its request names, in every library it is enabled in.
    Schedule,
    /// A hook: run when a user who connected it finishes playing something. Needs
    /// listening-activity, library-read where it was played, and personal settings to connect.
    Played,
    /// A hook: run when a user who connected it starts playing something. Needs what `Played`
    /// does.
    Playing,
    /// The searches of users who turned on sharing them with it.
    SearchActivity,
    /// A hook: run when a user who shares their searches settles on one, or asks for some to be
    /// forgotten. Needs search-activity, library-read, and personal settings to connect.
    Searched,
    /// A hook: run when albums are added, changed, or removed. Needs library-read too.
    AlbumsChanged,
    /// A hook: run when artists are added, changed, or removed. Needs library-read too.
    ArtistsChanged,
    /// Its rate limits that say `"per": "host"` are kept to by every plugin's requests to that
    /// host, so it can slow other plugins. Without it, they count its own.
    HostRateLimits,
}

impl Permission {
    pub const ALL: [Permission; 15] = [
        Self::LibraryRead,
        Self::LibraryAdd,
        Self::LibraryChange,
        Self::Network,
        Self::ListeningActivity,
        Self::TracksChanged,
        Self::ScanFinished,
        Self::Schedule,
        Self::Played,
        Self::Playing,
        Self::SearchActivity,
        Self::Searched,
        Self::AlbumsChanged,
        Self::ArtistsChanged,
        Self::HostRateLimits,
    ];

    /// Its name in manifests and the API, such as `libraryRead`.
    pub fn name(self) -> &'static str {
        match self {
            Self::LibraryRead => "libraryRead",
            Self::LibraryAdd => "libraryAdd",
            Self::LibraryChange => "libraryChange",
            Self::Network => "network",
            Self::ListeningActivity => "listeningActivity",
            Self::TracksChanged => "tracksChanged",
            Self::ScanFinished => "scanFinished",
            Self::Schedule => "schedule",
            Self::Played => "played",
            Self::Playing => "playing",
            Self::SearchActivity => "searchActivity",
            Self::Searched => "searched",
            Self::AlbumsChanged => "albumsChanged",
            Self::ArtistsChanged => "artistsChanged",
            Self::HostRateLimits => "hostRateLimits",
        }
    }

    pub fn from_name(name: &str) -> Option<Permission> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }

    /// Granted per library, rather than once for the plugin (`requirements/plugins.md` §4.1).
    pub fn per_library(self) -> bool {
        matches!(
            self,
            Self::LibraryRead
                | Self::LibraryAdd
                | Self::LibraryChange
                | Self::TracksChanged
                | Self::ScanFinished
                | Self::AlbumsChanged
                | Self::ArtistsChanged
        )
    }

    /// How the admin saw it when granting it.
    pub fn title(self) -> &'static str {
        match self {
            Self::LibraryRead => "Read the library",
            Self::LibraryAdd => "Add files to the library",
            Self::LibraryChange => "Change or delete files in the library",
            Self::Network => "Network access",
            Self::ListeningActivity => "Listening activity",
            Self::TracksChanged => "Run when tracks change",
            Self::ScanFinished => "Run when a scan finishes",
            Self::Schedule => "Run on a schedule",
            Self::Played => "Run when a connected user plays something",
            Self::Playing => "Run when a connected user starts playing something",
            Self::SearchActivity => "Search activity",
            Self::Searched => "Run when a user who shares their searches settles on one",
            Self::AlbumsChanged => "Run when albums change",
            Self::ArtistsChanged => "Run when artists change",
            Self::HostRateLimits => "Limit a service for every plugin",
        }
    }
}

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

/// A destination's rate limit as a manifest gives it: `"1/s"`, which counts the plugin's own
/// requests, or `{"limit": "1/s", "per": "user"}`, which says whose it counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Limit {
    Pace(String),
    Counted { limit: String, per: Per },
}

/// Whose requests to a destination a rate limit counts (`design/plugins.md` §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Per {
    /// The plugin's own, in every library and for every user.
    Plugin,
    /// The plugin's own in runs for one connected user, for a service that limits each
    /// account. A run for no user counts as the plugin's.
    User,
    /// Every plugin's, for a service that limits the whole server. Needs
    /// [`Permission::HostRateLimits`]; without it, it counts the plugin's own.
    Host,
}

impl Per {
    pub const ALL: [Per; 3] = [Per::Plugin, Per::User, Per::Host];

    pub fn name(self) -> &'static str {
        match self {
            Per::Plugin => "plugin",
            Per::User => "user",
            Per::Host => "host",
        }
    }
}

/// A rate limit a manifest declares, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimit {
    /// Lowercase, however the manifest gives it.
    pub host: String,
    /// As the manifest gives it, such as `"1/s"`.
    pub limit: String,
    /// The gap kept between requests.
    pub gap: Duration,
    pub per: Per,
}

/// The gap between requests that keeps to a rate limit: `"1/s"`, `"60/min"`, or `"1000/h"`.
/// Requests are spaced evenly rather than let through in bursts, which keeps within the limit
/// however the service counts it.
pub fn rate_limit_gap(limit: &str) -> Option<Duration> {
    let (count, per) = limit.split_once('/')?;
    let period = match per {
        "s" => 1,
        "min" => 60,
        "h" => 60 * 60,
        _ => return None,
    };
    // Digits only, as the pack tool reads them: no sign, no leading zero, and no more than nine.
    let digits = count.bytes().all(|b| b.is_ascii_digit()) && !count.starts_with('0');
    if !digits || count.is_empty() || count.len() > 9 {
        return None;
    }
    Some(Duration::from_secs(period) / count.parse::<u32>().ok()?)
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

/// Every problem with a manifest, in words an admin can act on; none when it is valid.
pub fn validate(manifest: &Value) -> Vec<String> {
    let Some(o) = manifest.as_object() else { return vec!["it must be a JSON object".into()] };
    let mut problems = Vec::new();
    let id_ok = o.get("id").and_then(Value::as_str).is_some_and(|id| {
        let mut chars = id.chars();
        (2..=64).contains(&id.len())
            && chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    });
    if !id_ok {
        problems.push(
            r#""id" must be lowercase letters, digits, and hyphens (2–64 characters)"#.into(),
        );
    }
    for (key, required) in
        [("name", true), ("version", true), ("description", false), ("author", false)]
    {
        let value = o.get(key);
        if (required || value.is_some()) && !non_empty(value) {
            problems.push(format!(r#""{key}" must be a non-empty string"#));
        }
    }
    if let Some(homepage) = o.get("homepage") {
        let url = homepage.as_str().unwrap_or("");
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            problems.push(r#""homepage" must be an http(s) URL"#.into());
        }
    }
    if o.get("apiVersion").and_then(Value::as_str) != Some(API_VERSION) {
        problems.push(format!(r#""apiVersion" must be "{API_VERSION}""#));
    }
    for key in ["settings", "personalSettings"] {
        if let Some(declared) = o.get(key) {
            problems.extend(settings::validate(key, declared));
        }
    }
    // A run sees both under one set of names.
    let names = |key| o.get(key).and_then(|s| s.get("properties")).and_then(Value::as_object);
    if let (Some(admin), Some(personal)) = (names("settings"), names("personalSettings")) {
        for name in personal.keys().filter(|name| admin.contains_key(*name)) {
            problems.push(format!(r#"personalSettings: "{name}" is also one of "settings""#));
        }
    }
    let Some(permissions) = o.get("permissions").and_then(Value::as_array) else {
        problems.push(r#""permissions" must be a list (empty if it needs none)"#.into());
        return problems;
    };
    let mut seen = Vec::new();
    for (i, request) in permissions.iter().enumerate() {
        let at = format!("permissions[{i}]");
        let Some(r) = request.as_object() else {
            problems.push(format!("{at} must be an object"));
            continue;
        };
        problems.extend(validate_request(&at, r, &mut seen));
    }
    // Limiting every plugin is what the permission is for, and only it allows that.
    let host = permissions
        .iter()
        .filter_map(|r| r.get("rateLimits")?.as_object())
        .any(|l| l.values().any(|limit| limit.get("per").and_then(Value::as_str) == Some("host")));
    let granted = seen.contains(&Permission::HostRateLimits);
    if host && !granted {
        problems.push(r#"a rate limit "per": "host" needs hostRateLimits as well"#.into());
    } else if granted && !host {
        problems.push(r#"hostRateLimits needs a rate limit "per": "host""#.into());
    }
    // A library hook's event is about the library, which only reading it can make anything of.
    let library_hooks = [
        Permission::TracksChanged,
        Permission::ScanFinished,
        Permission::AlbumsChanged,
        Permission::ArtistsChanged,
    ];
    for hook in library_hooks {
        if seen.contains(&hook) && !seen.contains(&Permission::LibraryRead) {
            problems.push(format!("{} needs libraryRead as well", hook.name()));
        }
    }
    // Their events are plays of a library's tracks or searches of it, by the users who
    // connected it.
    let hooks = [
        (Permission::Played, Permission::ListeningActivity),
        (Permission::Playing, Permission::ListeningActivity),
        (Permission::Searched, Permission::SearchActivity),
    ];
    for (hook, activity) in hooks.into_iter().filter(|(h, _)| seen.contains(h)) {
        let hook = hook.name();
        for needed in [Permission::LibraryRead, activity] {
            if !seen.contains(&needed) {
                problems.push(format!("{hook} needs {} as well", needed.name()));
            }
        }
        if o.get("personalSettings").is_none() {
            problems
                .push(format!(r#"{hook} needs "personalSettings", which users connect it with"#));
        }
    }
    problems
}

/// The problems with permission request `r`, found at `at` in the manifest. Adds its permission
/// to `seen`, so a later request for the same one is reported.
fn validate_request(at: &str, r: &Map<String, Value>, seen: &mut Vec<Permission>) -> Vec<String> {
    let Some(permission) =
        r.get("permission").and_then(Value::as_str).and_then(Permission::from_name)
    else {
        // As JSON.stringify shows it, so the pack tool words it the same.
        let shown = r.get("permission").map_or("undefined".to_owned(), Value::to_string);
        return vec![format!("{at}: unknown permission {shown}")];
    };
    let mut problems = Vec::new();
    if seen.contains(&permission) {
        problems.push(format!("{at}: {} is requested twice", permission.name()));
    }
    seen.push(permission);
    if !r.get("required").is_some_and(Value::is_boolean) {
        problems.push(format!(r#"{at}: "required" must be true or false"#));
    }
    if !non_empty(r.get("reason")) {
        problems.push(format!(r#"{at}: every permission needs a "reason" shown to the admin"#));
    }
    let destinations = r.get("destinations");
    if permission == Permission::Network {
        let hosts = destinations.and_then(Value::as_array);
        let valid = hosts.is_some_and(|hosts| {
            !hosts.is_empty() && hosts.iter().all(|h| h.as_str().is_some_and(|h| !h.is_empty()))
        });
        if !valid {
            problems.push(format!(
                r#"{at}: network needs "destinations", host names or ["*"] for any"#
            ));
        }
    } else if destinations.is_some() {
        problems.push(format!(r#"{at}: only network takes "destinations""#));
    }
    match r.get("rateLimits") {
        Some(limits) if permission == Permission::Network => {
            problems.extend(validate_rate_limits(at, limits, destinations));
        }
        Some(_) => problems.push(format!(r#"{at}: only network takes "rateLimits""#)),
        None => {}
    }
    let every = r.get("everyMinutes");
    if permission == Permission::Schedule {
        if !every
            .and_then(Value::as_u64)
            .is_some_and(|m| (MIN_EVERY_MINUTES..=u32::MAX as u64).contains(&m))
        {
            problems.push(format!(
                r#"{at}: schedule needs "everyMinutes", a whole number of minutes, at least {MIN_EVERY_MINUTES}"#
            ));
        }
    } else if every.is_some() {
        problems.push(format!(r#"{at}: only schedule takes "everyMinutes""#));
    }
    problems
}

fn validate_rate_limits(at: &str, limits: &Value, destinations: Option<&Value>) -> Vec<String> {
    let Some(limits) = limits.as_object() else {
        return vec![format!(
            r#"{at}: "rateLimits" must map destinations to limits such as "1/s""#
        )];
    };
    let hosts = destinations.and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let listed = || hosts.iter().filter_map(Value::as_str);
    let mut problems = Vec::new();
    for (host, limit) in limits {
        // Hosts are compared without case, so two keys that differ only in case would be one.
        let earlier = limits.keys().take_while(|key| *key != host);
        if let Some(same) = earlier.into_iter().find(|key| key.eq_ignore_ascii_case(host)) {
            problems.push(format!(r#"{at}: rateLimits: "{host}" is the same host as "{same}""#));
            continue;
        }
        let (pace, per) = match limit {
            Value::Object(o) => (o.get("limit"), o.get("per")),
            _ => (Some(limit), None),
        };
        let named = listed().any(|d| d.eq_ignore_ascii_case(host));
        if !is_host_name(host) {
            problems.push(format!(
                r#"{at}: rateLimits: "{host}" is not a host name; each limit is for one host"#
            ));
        } else if !named && !listed().any(|d| d == "*") {
            problems.push(format!(r#"{at}: rateLimits: "{host}" is not one of its destinations"#));
        } else if !named && per.and_then(Value::as_str) == Some("host") {
            // A limit on every plugin is one the admin approves by name.
            problems.push(format!(
                r#"{at}: rateLimits: "{host}" limits every plugin, so its destinations must name it rather than "*""#
            ));
        }
        if !pace.and_then(Value::as_str).is_some_and(|pace| rate_limit_gap(pace).is_some()) {
            problems.push(format!(
                r#"{at}: rateLimits: "{host}" needs a limit such as "1/s", "60/min", or "1000/h""#
            ));
        }
        let known = |per: &str| Per::ALL.iter().any(|p| p.name() == per);
        if limit.is_object() && !per.and_then(Value::as_str).is_some_and(known) {
            problems.push(format!(
                r#"{at}: rateLimits: "{host}" needs "per": "plugin", "user", or "host""#
            ));
        }
    }
    problems
}

/// Whether `host` is one host name, as a rate limit or an exemption from one names it. It is
/// matched exactly against the host a request names, so `*` or a pattern never would be.
pub fn is_host_name(host: &str) -> bool {
    let allowed = |b: u8| b.is_ascii_alphanumeric() || b == b'-' || b == b'.';
    !host.is_empty() && !host.ends_with('.') && host.bytes().all(allowed)
}

fn non_empty(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest valid component: the header, and nothing in it.
    const EMPTY_COMPONENT: [u8; 8] = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

    /// `EMPTY_COMPONENT` with a custom section `name` holding `data`.
    fn with_section(name: &str, data: &[u8]) -> Vec<u8> {
        let mut body = leb(name.len());
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(data);
        let mut bytes = EMPTY_COMPONENT.to_vec();
        bytes.push(0);
        bytes.extend(leb(body.len()));
        bytes.extend(body);
        bytes
    }

    /// `n` as unsigned LEB128, as section lengths are written.
    fn leb(mut n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    #[test]
    fn agrees_with_the_pack_tool_on_every_shared_case() {
        #[derive(Deserialize)]
        struct Case {
            name: String,
            manifest: Value,
            problems: Vec<String>,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../manifest-cases.json")).unwrap();
        assert!(cases.len() > 10);
        for mut case in cases {
            // A case leaves out `apiVersion` unless it is what the case is about.
            if let Some(manifest) = case.manifest.as_object_mut() {
                manifest.entry("apiVersion").or_insert(API_VERSION.into());
            }
            assert_eq!(validate(&case.manifest), case.problems, "{}", case.name);
        }
    }

    /// The shared cases are given `apiVersion` when they leave it out, so leaving it out is
    /// tested here.
    #[test]
    fn a_manifest_without_an_api_version_is_refused() {
        let mut manifest = serde_json::json!({"id": "ab", "name": "A", "version": "1",
            "apiVersion": API_VERSION, "permissions": []});
        assert_eq!(validate(&manifest), Vec::<String>::new());
        manifest.as_object_mut().unwrap().remove("apiVersion");
        assert_eq!(validate(&manifest), [r#""apiVersion" must be "0.1""#]);
    }

    #[test]
    fn reads_the_manifest_a_component_carries() {
        let manifest = serde_json::json!({"id": "lrclib-lyrics", "name": "LRCLIB Lyrics",
            "version": "0.1.0", "apiVersion": API_VERSION, "permissions": [{"permission": "network",
            "required": true, "reason": "To fetch lyrics.", "destinations": ["lrclib.net"]}]});
        let read = read(&with_section(SECTION, manifest.to_string().as_bytes())).unwrap();
        assert_eq!(read.id, "lrclib-lyrics");
        assert_eq!(read.destinations(), ["lrclib.net"]);
        assert_eq!(read.required().collect::<Vec<_>>(), [Permission::Network]);
    }

    #[test]
    fn rate_limits_become_even_gaps() {
        let gap = |limit| rate_limit_gap(limit).map(|gap| gap.as_millis());
        assert_eq!(gap("1/s"), Some(1000));
        assert_eq!(gap("4/s"), Some(250));
        assert_eq!(gap("60/min"), Some(1000));
        assert_eq!(gap("1000/h"), Some(3600));
        for wrong in ["0/s", "01/s", "+1/s", "1/sec", "1 /s", "/s", "1", "1000000000/s"] {
            assert_eq!(gap(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn rate_limits_say_whose_requests_they_count() {
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "id": "p", "name": "P", "version": "1", "apiVersion": API_VERSION,
            "permissions": [{"permission": "network", "required": true, "reason": "r",
                "destinations": ["a.example", "b.example", "c.example"],
                "rateLimits": {"A.Example": "2/s", "b.example": {"limit": "1/s", "per": "user"},
                    "c.example": {"limit": "60/min", "per": "host"}}}]
        }))
        .unwrap();
        let limit = |host: &str, limit: &str, ms, per| RateLimit {
            host: host.into(),
            limit: limit.into(),
            gap: Duration::from_millis(ms),
            per,
        };
        // Hosts come lowercase, however the manifest gives them.
        assert_eq!(
            manifest.rate_limits(),
            [
                limit("a.example", "2/s", 500, Per::Plugin),
                limit("b.example", "1/s", 1000, Per::User),
                limit("c.example", "60/min", 1000, Per::Host),
            ]
        );
    }

    #[test]
    fn says_why_a_file_is_not_a_plugin() {
        let message = |bytes: &[u8]| read(bytes).unwrap_err().0;
        assert!(message(b"hello").contains("not a WebAssembly file"));
        assert!(
            message(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0, 0, 0]).contains("module, not a component")
        );
        assert!(message(&EMPTY_COMPONENT).contains("no Jewelcase manifest"));
        assert!(message(&with_section(SECTION, b"{nope")).contains("not valid JSON"));
        assert!(message(&with_section(SECTION, b"{}")).starts_with("The manifest is invalid: "));
        let twice =
            [with_section(SECTION, b"{}"), with_section(SECTION, b"{}")[8..].to_vec()].concat();
        assert!(message(&twice).contains("more than one manifest"));
    }
}
