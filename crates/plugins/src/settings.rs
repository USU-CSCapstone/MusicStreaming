//! A plugin's settings (`requirements/plugins.md` §6): what its manifest declares, as a small
//! part of JSON Schema, and the checks on values an admin enters for them.
//!
//! Each setting is a string, number, whole number, or true or false, with an optional title,
//! description, list of choices (`enum`), and default. `writeOnly` marks a secret, which the
//! API never returns. `required` names the settings a plugin cannot work without.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    pub properties: BTreeMap<String, Setting>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    #[serde(rename = "type")]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, rename = "enum", skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// A secret: entered, used, and never shown again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub write_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    String,
    Number,
    Integer,
    Boolean,
}

impl Kind {
    fn from_name(name: &str) -> Option<Kind> {
        Some(match name {
            "string" => Kind::String,
            "number" => Kind::Number,
            "integer" => Kind::Integer,
            "boolean" => Kind::Boolean,
            _ => return None,
        })
    }

    fn accepts(self, value: &Value) -> bool {
        match self {
            Kind::String => value.is_string(),
            Kind::Number => value.is_number(),
            Kind::Integer => value.as_f64().is_some_and(|n| n.fract() == 0.0),
            Kind::Boolean => value.is_boolean(),
        }
    }

    fn described(self) -> &'static str {
        match self {
            Kind::String => "text",
            Kind::Number => "a number",
            Kind::Integer => "a whole number",
            Kind::Boolean => "true or false",
        }
    }
}

impl Setting {
    /// The name an admin knows it by: its title, or its name.
    fn label<'a>(&'a self, name: &'a str) -> &'a str {
        self.title.as_deref().unwrap_or(name)
    }
}

/// Every problem with a manifest's `settings`, in the words the pack tool uses too.
pub fn validate(settings: &Value) -> Vec<String> {
    let Some(properties) = settings.get("properties").and_then(Value::as_object) else {
        return vec![r#""settings" must be an object with "properties""#.into()];
    };
    let mut problems = Vec::new();
    for (name, setting) in properties {
        if !valid_name(name) {
            problems.push(format!(
                r#"settings: "{name}" is not a setting name (letters, digits, and _, starting with a letter)"#
            ));
            continue;
        }
        let at = format!("settings.{name}");
        let Some(s) = setting.as_object() else {
            problems.push(format!("{at} must be an object"));
            continue;
        };
        let Some(kind) = s.get("type").and_then(Value::as_str).and_then(Kind::from_name) else {
            problems.push(format!(r#"{at}: "type" must be string, number, integer, or boolean"#));
            continue;
        };
        for key in ["title", "description"] {
            if s.get(key).is_some_and(|v| !v.as_str().is_some_and(|v| !v.trim().is_empty())) {
                problems.push(format!(r#"{at}: "{key}" must be a non-empty string"#));
            }
        }
        if s.get("writeOnly").is_some_and(|v| !v.is_boolean()) {
            problems.push(format!(r#"{at}: "writeOnly" must be true or false"#));
        }
        if let Some(choices) = s.get("enum") {
            let valid = choices
                .as_array()
                .is_some_and(|c| !c.is_empty() && c.iter().all(|v| kind.accepts(v)));
            if !valid {
                problems.push(format!(
                    r#"{at}: "enum" must be a non-empty list of {} values"#,
                    name_of(kind)
                ));
            }
        }
        if s.get("default").is_some_and(|v| !kind.accepts(v)) {
            problems.push(format!(r#"{at}: "default" must be {}"#, kind.described()));
        }
    }
    if let Some(required) = settings.get("required") {
        let named = required.as_array().is_some_and(|r| {
            r.iter().all(|n| n.as_str().is_some_and(|n| properties.contains_key(n)))
        });
        if !named {
            problems.push(r#"settings: "required" must be a list of its settings' names"#.into());
        }
    }
    problems
}

fn name_of(kind: Kind) -> &'static str {
    match kind {
        Kind::String => "string",
        Kind::Number => "number",
        Kind::Integer => "integer",
        Kind::Boolean => "boolean",
    }
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Schema {
    /// Every problem with `values` entered for these settings: a name it does not declare, a
    /// value of the wrong kind, or one not among its choices.
    pub fn check(&self, values: &Map<String, Value>) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, value) in values {
            let Some(setting) = self.properties.get(name) else {
                problems.push(format!("{name} is not one of this plugin's settings"));
                continue;
            };
            let label = setting.label(name);
            if !setting.kind.accepts(value) {
                problems.push(format!("{label} must be {}", setting.kind.described()));
            } else if setting.choices.as_ref().is_some_and(|c| !c.contains(value)) {
                let choices: Vec<String> =
                    setting.choices.iter().flatten().map(Value::to_string).collect();
                problems.push(format!("{label} must be one of {}", choices.join(", ")));
            }
        }
        problems
    }

    /// The required settings `values` leaves without one, by the name an admin knows them by.
    pub fn missing(&self, values: &Map<String, Value>) -> Vec<String> {
        let unset = |name: &&String| {
            !values.contains_key(*name) && self.properties[*name].default.is_none()
        };
        self.required
            .iter()
            .filter(unset)
            .map(|name| self.properties[name].label(name).to_owned())
            .collect()
    }

    /// The value of each setting in `values`, or else its default.
    pub fn with_defaults(&self, values: &Map<String, Value>) -> Map<String, Value> {
        let defaults =
            self.properties.iter().filter_map(|(n, s)| Some((n.clone(), s.default.clone()?)));
        let mut effective: Map<String, Value> = defaults.collect();
        effective.extend(values.iter().map(|(n, v)| (n.clone(), v.clone())));
        effective
    }

    /// The names of the secrets among them.
    pub fn secrets(&self) -> impl Iterator<Item = &str> {
        self.properties.iter().filter(|(_, s)| s.write_only).map(|(n, _)| n.as_str())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schema() -> Schema {
        serde_json::from_value(json!({
            "properties": {
                "apiKey": { "type": "string", "title": "API key", "writeOnly": true },
                "mode": { "type": "string", "enum": ["fast", "careful"], "default": "fast" },
                "limit": { "type": "integer", "title": "Limit" },
                "syncedOnly": { "type": "boolean" }
            },
            "required": ["apiKey", "mode"]
        }))
        .unwrap()
    }

    fn values(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn checks_kinds_choices_and_names() {
        let s = schema();
        assert!(
            s.check(&values(json!({ "apiKey": "k", "limit": 3, "syncedOnly": true }))).is_empty()
        );
        assert_eq!(
            s.check(&values(
                json!({ "limit": 2.5, "mode": "slow", "syncedOnly": "yes", "other": 1 })
            )),
            [
                "Limit must be a whole number",
                r#"mode must be one of "fast", "careful""#,
                "other is not one of this plugin's settings",
                "syncedOnly must be true or false",
            ]
        );
    }

    #[test]
    fn a_required_setting_with_a_default_is_never_missing() {
        let s = schema();
        assert_eq!(s.missing(&values(json!({}))), ["API key"]);
        assert!(s.missing(&values(json!({ "apiKey": "k" }))).is_empty());
        assert_eq!(
            s.with_defaults(&values(json!({ "apiKey": "k" }))),
            values(json!({ "apiKey": "k", "mode": "fast" }))
        );
        assert_eq!(s.secrets().collect::<Vec<_>>(), ["apiKey"]);
    }
}
