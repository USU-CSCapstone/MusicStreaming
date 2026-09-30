//! The manifest a plugin file carries (`design/plugins.md` §6): UTF-8 JSON in a top-level
//! custom section named `jewelcase:manifest` of a WebAssembly component.
//!
//! This is the authority on what installs. `tools/plugin-pack` validates the same way for
//! authors, and both are held to `manifest-cases.json`, so they cannot drift apart.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use wasmparser::{Encoding, Parser, Payload};

pub const SECTION: &str = "jewelcase:manifest";
pub const API_VERSION: &str = "0.1";

/// What a plugin can ask for (`requirements/plugins.md` §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    LibraryRead,
    LibraryWrite,
    Network,
    ListeningActivity,
}

impl Permission {
    pub const ALL: [Permission; 4] =
        [Self::LibraryRead, Self::LibraryWrite, Self::Network, Self::ListeningActivity];

    /// Its name in manifests and the API, such as `libraryRead`.
    pub fn name(self) -> &'static str {
        match self {
            Self::LibraryRead => "libraryRead",
            Self::LibraryWrite => "libraryWrite",
            Self::Network => "network",
            Self::ListeningActivity => "listeningActivity",
        }
    }

    pub fn from_name(name: &str) -> Option<Permission> {
        Self::ALL.into_iter().find(|p| p.name() == name)
    }

    /// Granted per library, rather than once for the plugin (`requirements/plugins.md` §4.1).
    pub fn per_library(self) -> bool {
        matches!(self, Self::LibraryRead | Self::LibraryWrite)
    }

    /// How the admin saw it when granting it.
    pub fn title(self) -> &'static str {
        match self {
            Self::LibraryRead => "Read the library",
            Self::LibraryWrite => "Write to the library",
            Self::Network => "Network access",
            Self::ListeningActivity => "Listening activity",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub permissions: Vec<PermissionRequest>,
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
}

impl Manifest {
    /// The hosts its network request names; none if it asks for no network.
    pub fn destinations(&self) -> &[String] {
        let network = self.permissions.iter().find(|r| r.permission == Permission::Network);
        network.and_then(|r| r.destinations.as_deref()).unwrap_or_default()
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
    problems
}

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
    problems
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
        for case in cases {
            assert_eq!(validate(&case.manifest), case.problems, "{}", case.name);
        }
    }

    #[test]
    fn reads_the_manifest_a_component_carries() {
        let manifest = r#"{"id": "lrclib-lyrics", "name": "LRCLIB Lyrics", "version": "0.1.0",
            "apiVersion": "0.1", "permissions": [{"permission": "network", "required": true,
            "reason": "To fetch lyrics.", "destinations": ["lrclib.net"]}]}"#;
        let read = read(&with_section(SECTION, manifest.as_bytes())).unwrap();
        assert_eq!(read.id, "lrclib-lyrics");
        assert_eq!(read.destinations(), ["lrclib.net"]);
        assert_eq!(read.required().collect::<Vec<_>>(), [Permission::Network]);
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
