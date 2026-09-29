//! IDs as the API shows them: the random 63-bit database ID as a decimal string
//! (`design/database.md` §1). Clients treat them as opaque.

use serde::{Serialize, Serializer};

use super::{Code, Problem};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id(pub i64);

impl Id {
    /// Reads an ID from a request path. Anything that is not one answers `404`, exactly like an
    /// ID that is out of the caller's reach (`requirements/users.md` §10).
    pub fn parse(text: &str) -> Result<Id, Problem> {
        Id::canonical(text).ok_or_else(|| Problem::new(Code::NotFound))
    }

    /// Reads an ID in its one canonical spelling: no sign, no leading zeros.
    pub fn canonical(text: &str) -> Option<Id> {
        text.parse::<i64>()
            .ok()
            .filter(|id| *id >= 0 && id.to_string() == text)
            .map(Id)
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
        assert_eq!(Id::parse(json.trim_matches('"')).unwrap(), id);
    }

    #[test]
    fn anything_else_is_not_found() {
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
            assert!(Id::parse(text).is_err(), "{text:?}");
        }
    }
}
