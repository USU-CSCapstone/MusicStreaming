//! The names a scan's trigger and state and a problem's kind have in the API and in the schema's
//! `CHECK`s. Each enum's serde derive spells them, so the spelling is written once, beside the
//! variants, and the store writes and reads exactly those names; the test below holds them to
//! the schema.

use rusqlite::Row;
use rusqlite::types::Type;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// The name of `value`, a scan's trigger or state or a problem's kind. Panics if `value` does not
/// serialize as a string, as only a unit variant does.
pub fn name(value: impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => unreachable!("a unit variant serializes as its name"),
    }
}

/// The trigger, state or kind named in column `index`. A name serde does not know is a
/// conversion error.
pub fn parse<T: DeserializeOwned>(row: &Row, index: usize) -> rusqlite::Result<T> {
    serde_json::from_value(serde_json::Value::String(row.get(index)?)).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

#[cfg(test)]
mod tests {
    use jewelcase_scanner::{ProblemKind, ScanState, Trigger};

    use super::name;

    /// The names `0001_libraries.sql` allows in `scans.trigger`, `scans.state` and
    /// `scan_problem_groups.kind`.
    #[test]
    fn every_name_is_one_the_schema_allows() {
        use ProblemKind::*;
        use ScanState::*;
        use Trigger::*;
        assert_eq!(
            [Initial, Watch, Scheduled, Manual, Reconfigure, Restore].map(name),
            ["initial", "watch", "scheduled", "manual", "reconfigure", "restore"]
        );
        assert_eq!(
            [Queued, Running, Completed, Cancelled, Suspended].map(name),
            ["queued", "running", "completed", "cancelled", "suspended"]
        );
        assert_eq!(
            [
                Unreadable,
                PermissionDenied,
                MalformedTags,
                UnsupportedEncoding,
                CorruptAudio,
                Stalled
            ]
            .map(name),
            [
                "unreadable",
                "permissionDenied",
                "malformedTags",
                "unsupportedEncoding",
                "corruptAudio",
                "stalled"
            ]
        );
    }
}
