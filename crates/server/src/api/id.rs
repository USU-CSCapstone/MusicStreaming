//! IDs as the API shows them: the random 63-bit database ID as a decimal string
//! (`design/database.md` §1). Clients treat them as opaque. Also the UUIDs clients choose for
//! what they write.

use serde::{Deserialize, Serialize};

pub use jewelcase_catalog::{AnyId, Id};

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
