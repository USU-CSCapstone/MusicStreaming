//! Column types for values stored through their serde form: [`Json`] for a JSON column and
//! [`Text`] for an enum stored as its name. Each reads and writes the one spelling serde gives
//! it, and a value that does not read as its type is an error, never a default.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::Serialize;
use serde::de::value::StrDeserializer;
use serde::de::{DeserializeOwned, IntoDeserializer};
use serde_json::Value;

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

/// A column holding an enum's variant as the name serde gives it, such as `permissionDenied`:
/// read as `row.get::<_, Text<ProblemKind>>(2)?.0`, written as `params![Text(kind)]`. `T` is an
/// enum of unit variants.
pub struct Text<T>(pub T);

impl<T: DeserializeOwned> FromSql for Text<T> {
    /// Reads the variant the column names. A name serde does not know is a conversion error.
    fn column_result(value: ValueRef) -> FromSqlResult<Self> {
        let name: StrDeserializer<serde::de::value::Error> = value.as_str()?.into_deserializer();
        T::deserialize(name).map(Text).map_err(FromSqlError::other)
    }
}

impl<T: Serialize> ToSql for Text<T> {
    /// Writes the variant's name, or fails the statement if `T` serializes as anything other
    /// than a string, as only a unit variant does.
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        match serde_json::to_value(&self.0) {
            Ok(Value::String(name)) => Ok(ToSqlOutput::from(name)),
            Ok(other) => Err(rusqlite::Error::ToSqlConversionFailure(
                format!("{other} is not a variant's name").into(),
            )),
            Err(error) => Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use jewelcase_scanner::{ProblemKind, ScanState, Trigger};
    use rusqlite::Connection;
    use serde::Deserialize;

    use super::{Json, Text};
    use crate::db::feed::{Entity, Op};
    use crate::db::testing::stored;

    #[derive(Debug, PartialEq, Deserialize)]
    #[serde(rename_all = "camelCase")]
    enum Kind {
        PlainName,
    }

    /// Reads `sql`'s one value as `T`.
    fn read<T: rusqlite::types::FromSql>(sql: &str) -> rusqlite::Result<T> {
        Connection::open_in_memory().unwrap().query_row(sql, [], |row| row.get(0))
    }

    #[test]
    fn json_reads_back_as_written() {
        let conn = Connection::open_in_memory().unwrap();
        let written = vec!["a".to_owned(), "b\"c".to_owned()];
        let read: Json<Vec<String>> =
            conn.query_row("SELECT ?1", [Json(&written)], |row| row.get(0)).unwrap();
        assert_eq!(read.0, written);
        assert_eq!(stored(Json(&written)), r#"["a","b\"c"]"#);
    }

    /// Text that is not JSON, JSON SQLite accepts but serde cannot read, such as a number out of
    /// range, and JSON of another shape are each an error naming the column.
    #[test]
    fn json_that_does_not_read_as_its_type_is_an_error() {
        for sql in ["SELECT 0, 'no'", r#"SELECT 0, '{"key": 1e400}'"#, "SELECT 0, '[1]'"] {
            let read = Connection::open_in_memory().unwrap().query_row(sql, [], |row| {
                row.get::<_, Json<serde_json::Map<String, serde_json::Value>>>(1).map(|json| json.0)
            });
            assert!(
                matches!(read, Err(rusqlite::Error::FromSqlConversionFailure(1, ..))),
                "{sql}: {read:?}"
            );
        }
    }

    #[test]
    fn json_of_a_value_with_no_json_form_is_an_error() {
        let pairs = BTreeMap::from([((1, 2), 3)]);
        let conn = Connection::open_in_memory().unwrap();
        let written = conn.query_row("SELECT ?1", [Json(pairs)], |row| row.get::<_, String>(0));
        assert!(matches!(written, Err(rusqlite::Error::ToSqlConversionFailure(_))), "{written:?}");
    }

    #[test]
    fn text_reads_a_variant_by_its_serde_name_and_nothing_else() {
        assert_eq!(read::<Text<Kind>>("SELECT 'plainName'").unwrap().0, Kind::PlainName);
        for unknown in ["SELECT 'PlainName'", "SELECT 'plain_name'", "SELECT 1"] {
            assert!(read::<Text<Kind>>(unknown).is_err(), "{unknown}");
        }
    }

    #[test]
    fn text_of_a_value_that_is_not_a_name_is_an_error() {
        let conn = Connection::open_in_memory().unwrap();
        let written = conn.query_row("SELECT ?1", [Text(3)], |row| row.get::<_, String>(0));
        assert!(matches!(written, Err(rusqlite::Error::ToSqlConversionFailure(_))), "{written:?}");
    }

    /// The names `0001_libraries.sql` allows in `scans.trigger`, `scans.state` and
    /// `scan_problem_groups.kind`, and `0004_feeds.sql` in `library_changes`, as each is stored.
    /// A scan's trigger and state are read back too.
    #[test]
    fn every_stored_name_is_one_the_schema_allows() {
        use ProblemKind::*;
        use ScanState::*;
        use Trigger::*;
        let triggers = [Initial, Watch, Scheduled, Manual, Reconfigure, Restore];
        let names = ["initial", "watch", "scheduled", "manual", "reconfigure", "restore"];
        assert_eq!(triggers.map(|t| stored(Text(t))), names);
        for (trigger, name) in triggers.into_iter().zip(names) {
            assert_eq!(read::<Text<Trigger>>(&format!("SELECT '{name}'")).unwrap().0, trigger);
        }
        let states = [Queued, Running, Completed, Cancelled, Suspended];
        let names = ["queued", "running", "completed", "cancelled", "suspended"];
        assert_eq!(states.map(|s| stored(Text(s))), names);
        for (state, name) in states.into_iter().zip(names) {
            assert_eq!(read::<Text<ScanState>>(&format!("SELECT '{name}'")).unwrap().0, state);
        }
        let kinds = [
            Unreadable,
            PermissionDenied,
            MalformedTags,
            UnsupportedEncoding,
            CorruptAudio,
            Stalled,
        ];
        let names = [
            "unreadable",
            "permissionDenied",
            "malformedTags",
            "unsupportedEncoding",
            "corruptAudio",
            "stalled",
        ];
        assert_eq!(kinds.map(|k| stored(Text(k))), names);
        let entities = [Entity::Library, Entity::Track, Entity::Album, Entity::Artist, Entity::Tag];
        assert_eq!(
            entities.map(|e| stored(Text(e))),
            ["library", "track", "album", "artist", "tag"]
        );
        assert_eq!([Op::Upsert, Op::Delete].map(|o| stored(Text(o))), ["upsert", "delete"]);
    }
}
