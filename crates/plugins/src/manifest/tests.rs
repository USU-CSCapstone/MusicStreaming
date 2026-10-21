//! Reading the manifest a component carries, and the problems `validate` finds in one.

use std::time::Duration;

use super::rate_limit::LIMIT_KEYS;
use super::validate::{KEYS, REQUEST_KEYS};
use super::*;
use crate::settings;

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

/// Each case in `manifest-cases.json` is a manifest and every problem `validate` finds in
/// it, in order. A new rule gets a case there.
#[test]
fn finds_the_problems_each_case_lists() {
    #[derive(Deserialize)]
    struct Case {
        name: String,
        manifest: Value,
        problems: Vec<String>,
    }
    let cases: Vec<Case> = serde_json::from_str(include_str!("../../manifest-cases.json")).unwrap();
    assert!(cases.len() > 10);
    for case in cases {
        assert_eq!(validate(&case.manifest), case.problems, "{}", case.name);
    }
}

/// Each permission's name reads back as it. A permission name in the cases is refused as unknown
/// exactly when it names none, and the names they accept cover all of [`Permission::ALL`].
#[test]
fn every_permission_is_named_as_the_cases_name_it() {
    for permission in Permission::ALL {
        assert_eq!(Permission::from_name(&permission.name()), Some(permission));
    }
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../manifest-cases.json")).unwrap();
    let mut asked = Vec::new();
    for case in &cases {
        let problems = case["problems"].as_array().unwrap();
        let requests = case["manifest"]["permissions"].as_array().into_iter().flatten();
        let named = requests.enumerate().filter_map(|(i, r)| Some((i, r["permission"].as_str()?)));
        for (i, name) in named {
            let unknown = format!(r#"permissions[{i}]: unknown permission "{name}""#);
            let refused = problems.contains(&unknown.into());
            assert_eq!(Permission::from_name(name).is_none(), refused, "{name}");
            if !refused {
                asked.push(name.to_owned());
            }
        }
    }
    asked.sort();
    asked.dedup();
    let mut all = Permission::ALL.map(Permission::name);
    all.sort();
    assert_eq!(asked, all);
}

/// Every key validation allows, at every level, is one the typed form reads and writes back,
/// so none is accepted and then dropped.
#[test]
fn the_typed_form_keeps_every_key_validation_allows() {
    let setting = serde_json::json!({"type": "integer", "title": "Level", "description": "How.",
        "enum": [1, 2], "default": 1, "writeOnly": true});
    let schema = serde_json::json!({"properties": {"level": setting}, "required": ["level"]});
    let mut personal = schema.clone();
    personal["properties"] = serde_json::json!({"mine": setting});
    personal["required"] = serde_json::json!(["mine"]);
    let limit = serde_json::json!({"limit": "1/s", "per": "user"});
    let network = serde_json::json!({"permission": "network", "required": true, "reason": "r",
        "destinations": ["a.example", "b.example"],
        "rateLimits": {"a.example": "2/s", "b.example": limit}});
    let schedule = serde_json::json!({"permission": "schedule", "required": false,
        "reason": "r", "everyMinutes": 60});
    let manifest = serde_json::json!({"id": "ab", "name": "A", "version": "1",
        "apiVersion": API_VERSION, "description": "D", "author": "Me",
        "homepage": "https://a.example", "permissions": [network, schedule],
        "settings": schema, "personalSettings": personal});
    assert_eq!(validate(&manifest), Vec::<String>::new());

    let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    let sorted = |known: &[&str]| {
        let mut known: Vec<String> = known.iter().map(|k| k.to_string()).collect();
        known.sort();
        known
    };
    let mut request = [keys(&network), keys(&schedule)].concat();
    request.sort();
    request.dedup();
    assert_eq!(keys(&manifest), sorted(&KEYS));
    assert_eq!(request, sorted(&REQUEST_KEYS));
    assert_eq!(keys(&limit), sorted(&LIMIT_KEYS));
    assert_eq!(keys(&schema), sorted(&settings::SCHEMA_KEYS));
    assert_eq!(keys(&setting), sorted(&settings::SETTING_KEYS));

    let typed: Manifest = serde_json::from_value(manifest.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), manifest);
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
    assert!(message(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0, 0, 0]).contains("module, not a component"));
    assert!(message(&EMPTY_COMPONENT).contains("no Jewelcase manifest"));
    assert!(message(&with_section(SECTION, b"{nope")).contains("not valid JSON"));
    assert!(message(&with_section(SECTION, b"{}")).starts_with("The manifest is invalid: "));
    let twice = [with_section(SECTION, b"{}"), with_section(SECTION, b"{}")[8..].to_vec()].concat();
    assert!(message(&twice).contains("more than one manifest"));
}
