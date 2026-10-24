//! Strings and lists a request may send only so much of: the spec's `maxLength` and `maxItems`.
//! Past its limit one is refused as it is read, so the request answers `422 validation_failed`
//! like any other body that does not fit.

use serde::{Deserialize, Serialize};

/// A string of at most `MAX` characters, counted as the spec's `maxLength` counts them. Written
/// back as the plain string.
#[derive(Deserialize, Serialize)]
#[serde(try_from = "String")]
pub struct BoundedString<const MAX: usize>(String);

impl<const MAX: usize> TryFrom<String> for BoundedString<MAX> {
    type Error = String;

    /// `text`, unless it is longer than `MAX` characters.
    fn try_from(text: String) -> Result<Self, String> {
        if text.chars().count() > MAX {
            return Err(format!("a string longer than {MAX} characters"));
        }
        Ok(BoundedString(text))
    }
}

/// A list of at most `MAX` items. Written back as the plain list.
#[derive(Deserialize, Serialize)]
#[serde(try_from = "Vec<T>")]
pub struct BoundedVec<T, const MAX: usize>(pub Vec<T>);

impl<T, const MAX: usize> TryFrom<Vec<T>> for BoundedVec<T, MAX> {
    type Error = String;

    /// `items`, unless there are more than `MAX` of them.
    fn try_from(items: Vec<T>) -> Result<Self, String> {
        if items.len() > MAX {
            return Err(format!("a list of more than {MAX} items"));
        }
        Ok(BoundedVec(items))
    }
}
