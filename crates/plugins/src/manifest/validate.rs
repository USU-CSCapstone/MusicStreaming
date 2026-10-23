//! Checking a manifest's JSON, every key at every level, before it is read into its typed form.

use serde_json::{Map, Value};

use super::rate_limit::validate_rate_limits;
use super::{API_VERSION, Hook, MIN_EVERY_MINUTES, Permission, is_host_name, unknown_keys};
use crate::settings;

/// The keys a manifest may give at its top level.
pub(super) const KEYS: [&str; 10] = [
    "id",
    "name",
    "version",
    "apiVersion",
    "description",
    "author",
    "homepage",
    "permissions",
    "settings",
    "personalSettings",
];

/// The keys a permission request may give.
pub(super) const REQUEST_KEYS: [&str; 6] =
    ["permission", "required", "reason", "destinations", "rateLimits", "everyMinutes"];

/// Every problem with a manifest, in words an admin can act on; none when it is valid.
pub fn validate(manifest: &Value) -> Vec<String> {
    let Some(o) = manifest.as_object() else { return vec!["it must be a JSON object".into()] };
    let mut problems = unknown_keys("", o, &KEYS);
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
    // What each one needs, in the order they were asked for. A personal hook runs for the users
    // who connected the plugin, and they connect it by entering its personal settings.
    for permission in &seen {
        let name = permission.name();
        for needed in permission.needs().iter().filter(|needed| !seen.contains(needed)) {
            problems.push(format!("{name} needs {} as well", needed.name()));
        }
        if permission.hook() == Some(Hook::Personal) && o.get("personalSettings").is_none() {
            problems
                .push(format!(r#"{name} needs "personalSettings", which users connect it with"#));
        }
    }
    problems
}

/// The problems with permission request `r`, found at `at` in the manifest. Adds its permission
/// to `seen` unless it is there already, so a later request for the same one is reported.
fn validate_request(at: &str, r: &Map<String, Value>, seen: &mut Vec<Permission>) -> Vec<String> {
    let Some(permission) =
        r.get("permission").and_then(Value::as_str).and_then(Permission::from_name)
    else {
        return vec![match r.get("permission") {
            Some(shown) => format!("{at}: unknown permission {shown}"),
            None => format!(r#"{at}: "permission" is missing"#),
        }];
    };
    let mut problems = unknown_keys(at, r, &REQUEST_KEYS);
    if seen.contains(&permission) {
        problems.push(format!("{at}: {} is requested twice", permission.name()));
    } else {
        seen.push(permission);
    }
    if !r.get("required").is_some_and(Value::is_boolean) {
        problems.push(format!(r#"{at}: "required" must be true or false"#));
    }
    if !non_empty(r.get("reason")) {
        problems.push(format!(r#"{at}: every permission needs a "reason" shown to the admin"#));
    }
    let destinations = r.get("destinations");
    if permission == Permission::Network {
        let hosts = destinations
            .and_then(Value::as_array)
            .and_then(|hosts| hosts.iter().map(Value::as_str).collect::<Option<Vec<_>>>());
        match hosts {
            Some(hosts) if !hosts.is_empty() && !hosts.contains(&"") => {
                let named = hosts.into_iter().filter(|h| *h != "*" && !is_host_name(h));
                problems.extend(
                    named.map(|h| format!(r#"{at}: destinations: "{h}" is not a host name"#)),
                );
            }
            _ => problems.push(format!(
                r#"{at}: network needs "destinations", host names or ["*"] for any"#
            )),
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

/// Whether `value` is a string with something in it but whitespace.
fn non_empty(value: Option<&Value>) -> bool {
    value.and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty())
}
