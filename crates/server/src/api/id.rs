//! IDs as the API shows them: the random 63-bit database ID as a decimal string
//! (`design/database.md` §1). Clients treat them as opaque. Also the UUIDs clients choose for
//! what they write.

use rusqlite::ToSql;
use rusqlite::types::ToSqlOutput;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A database ID, shown as a decimal string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id(pub i64);

impl Id {
    /// Reads an ID in its one canonical spelling: no sign, no leading zeros.
    pub fn canonical(text: &str) -> Option<Id> {
        text.parse::<i64>().ok().filter(|id| *id >= 0 && id.to_string() == text).map(Id)
    }
}

/// From a request path (`extract::Path`), where anything that is not an ID answers `404`.
impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Id, D::Error> {
        let text = String::deserialize(deserializer)?;
        Id::canonical(&text).ok_or_else(|| D::Error::custom("not an ID"))
    }
}

/// As a decimal string.
impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

/// An ID a request names in its body, its query, or its path as `authenticate` reads it, where
/// one that is not an ID names nothing, exactly as an unknown one does
/// (`requirements/users.md` §10): `None`. Bound as SQL, `None` is `NULL`, which `id = ?` never
/// matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnyId(pub Option<i64>);

impl From<&str> for AnyId {
    /// Reads `text` as [`Id::canonical`] does, with anything else as `None`.
    fn from(text: &str) -> AnyId {
        AnyId(Id::canonical(text).map(|Id(id)| id))
    }
}

/// From a JSON string or a query parameter. Anything but text is refused.
impl<'de> Deserialize<'de> for AnyId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<AnyId, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(AnyId::from(text.as_str()))
    }
}

/// As the ID it names, or `null` for one that names nothing.
impl Serialize for AnyId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.map(Id).serialize(serializer)
    }
}

impl ToSql for AnyId {
    /// The ID, or `NULL` for one that names nothing.
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

/// A UUID a client chose (`format: uuid`), as `playId` and `commandId` are, in lowercase
/// whatever case it was sent in, so that one ID names one thing (RFC 9562 §4). Anything else
/// is refused as it is read, which answers `422`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "String")]
pub struct Uuid(String);

impl TryFrom<String> for Uuid {
    type Error = &'static str;

    /// `text` in lowercase, if it is a UUID: 32 hex digits in groups of 8, 4, 4, 4 and 12,
    /// joined by hyphens.
    fn try_from(mut text: String) -> Result<Uuid, &'static str> {
        let uuid = text.len() == 36
            && text.char_indices().all(|(i, c)| match i {
                8 | 13 | 18 | 23 => c == '-',
                _ => c.is_ascii_hexdigit(),
            });
        if !uuid {
            return Err("must be a UUID");
        }
        text.make_ascii_lowercase();
        Ok(Uuid(text))
    }
}

impl Uuid {
    /// The UUID's text, in lowercase.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn round_trips_as_a_decimal_string() {
        let id = Id(6_679_900_963_316_241_941);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"6679900963316241941\"");
        assert_eq!(serde_json::from_str::<Id>(&json).unwrap(), id);
    }

    #[test]
    fn anything_else_is_not_an_id() {
        for text in ["", "abc", "-1", "+1", "01", " 1", "1.0", "9223372036854775808"] {
            let json = serde_json::Value::from(text);
            assert!(serde_json::from_value::<Id>(json).is_err(), "{text:?}");
        }
    }

    #[test]
    fn any_id_names_nothing_for_text_that_is_not_an_id() {
        let read = |json| serde_json::from_value::<AnyId>(json).ok();
        assert_eq!(read(json!("42")), Some(AnyId(Some(42))));
        for text in ["", "abc", "-1", "01", "9223372036854775808"] {
            assert_eq!(read(json!(text)), Some(AnyId(None)), "{text:?}");
        }
        assert_eq!(read(json!(42)), None, "a number is not text");
        assert_eq!(serde_json::to_value(AnyId(Some(42))).unwrap(), json!("42"));
    }

    #[test]
    fn a_uuid_is_read_in_lowercase_and_anything_else_is_refused() {
        let read = |text: &str| serde_json::from_value::<Uuid>(json!(text)).ok();
        let uuid = "5f0c6a52-3b7e-4d43-9a8e-0d1c2b3a4f5e";
        assert_eq!(read(&uuid.to_uppercase()).as_ref().map(Uuid::as_str), Some(uuid));
        assert_eq!(serde_json::to_value(read(uuid).unwrap()).unwrap(), json!(uuid));
        let (underscored, longer) = (uuid.replace('-', "_"), format!("{uuid}0"));
        for text in ["", "not-a-uuid", &uuid[1..], underscored.as_str(), longer.as_str()] {
            assert_eq!(read(text), None, "{text:?}");
        }
        assert_eq!(read("5f0c6a52-3b7e-4d43-9a8e-0d1c2b3a4f5g"), None, "not hex");
    }
}
