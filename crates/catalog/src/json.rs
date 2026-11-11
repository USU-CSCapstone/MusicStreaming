//! A column holding JSON, read and written through its serde form.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// A column holding JSON, read as `T`, as in `row.get::<_, Json<Vec<String>>>(2)?.0`, and
/// written as compact JSON, as in `params![Json(&excludes)]`.
pub struct Json<T>(pub T);

impl<T: DeserializeOwned> FromSql for Json<T> {
    /// Reads the column as JSON for `T`. Text that is not JSON, or JSON of another shape, is a
    /// conversion error.
    fn column_result(value: ValueRef) -> FromSqlResult<Self> {
        serde_json::from_slice(value.as_bytes()?).map(Json).map_err(FromSqlError::other)
    }
}

impl<T: Serialize> ToSql for Json<T> {
    /// Writes the value as compact JSON text, or fails the statement if it does not serialize.
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        serde_json::to_string(&self.0)
            .map(ToSqlOutput::from)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
    }
}
