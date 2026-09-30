//! IDs as the API shows them: the random 63-bit database ID as a decimal string
//! (`design/database.md` §1). Clients treat them as opaque.

use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id(pub i64);

impl Id {
    /// Reads an ID in its one canonical spelling: no sign, no leading zeros.
    pub fn canonical(text: &str) -> Option<Id> {
        text.parse::<i64>()
            .ok()
            .filter(|id| *id >= 0 && id.to_string() == text)
            .map(Id)
    }
}

/// From a request path (`extract::Path`), where anything that is not an ID answers `404`.
impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Id, D::Error> {
        let text = <std::borrow::Cow<str>>::deserialize(deserializer)?;
        Id::canonical(&text).ok_or_else(|| D::Error::custom("not an ID"))
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
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
        for text in [
            "",
            "abc",
            "-1",
            "+1",
            "01",
            " 1",
            "1.0",
            "9223372036854775808",
        ] {
            let json = serde_json::Value::from(text);
            assert!(serde_json::from_value::<Id>(json).is_err(), "{text:?}");
        }
    }
}
