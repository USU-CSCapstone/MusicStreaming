//! The names a play's end and origin have in the API and in `plays` (`0005_activity.sql`).

use serde::{Deserialize, Serialize};

/// How a play ended: the spec's `end`, stored by the same name in `plays.ended`.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum End {
    Finished,
    Skipped,
    Stopped,
}

/// What started a play: the spec's `origin`, stored by the same name in `plays.origin`.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Manual,
    Context,
    Automatic,
}

#[cfg(test)]
mod tests {
    use super::{End, Origin};
    use crate::db::Text;
    use crate::db::testing::stored;

    /// The names `0005_activity.sql` allows in `plays.ended` and `plays.origin`, as each is
    /// stored.
    #[test]
    fn every_end_and_origin_is_stored_by_the_name_the_schema_allows() {
        let ends = [End::Finished, End::Skipped, End::Stopped].map(|end| stored(Text(end)));
        assert_eq!(ends, ["finished", "skipped", "stopped"]);
        let origins = [Origin::Manual, Origin::Context, Origin::Automatic];
        assert_eq!(origins.map(|origin| stored(Text(origin))), ["manual", "context", "automatic"]);
    }
}
