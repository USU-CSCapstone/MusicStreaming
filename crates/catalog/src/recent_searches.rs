//! Recent searches as `listRecentSearches` and `recordRecentSearch` answer them
//! (`requirements/search.md` §6). The server and a device read the same `recent_searches`, which
//! a device keeps from its account feed, so both answer alike. Writing them is
//! [`crate::mutate`]'s.

use rusqlite::{Connection, Row, params};
use serde::Serialize;

use crate::Id;
use crate::time::serialize_rfc3339;

/// The most recent searches kept per user and library.
pub const KEPT: i64 = 20;

/// The kinds of result a search returns, and the table each is in.
pub const KINDS: [(&str, &str); 3] =
    [("track", "tracks"), ("album", "albums"), ("artist", "artists")];

/// The spec's `RecentSearch`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentSearch {
    id: Id,
    query: String,
    selected: Option<EntityRef>,
    #[serde(serialize_with = "serialize_rfc3339")]
    searched_at: i64,
}

/// The spec's `EntityRef`, for the kinds of result search returns.
#[derive(Debug, Serialize)]
pub struct EntityRef {
    #[serde(rename = "type")]
    kind: &'static str,
    id: Id,
}

/// `listRecentSearches`' answer.
#[derive(Debug, Serialize)]
pub struct RecentSearches {
    items: Vec<RecentSearch>,
}

/// What [`row`] reads, in order, and the table it reads them from.
const COLUMNS: &str = "id, query, selected_type, selected_id, searched_at FROM recent_searches";

/// Reads a [`COLUMNS`] row. A selected result of a type no longer in [`KINDS`] reads as none.
fn row(row: &Row) -> rusqlite::Result<RecentSearch> {
    let kind: Option<String> = row.get(2)?;
    let kind = kind.and_then(|kind| KINDS.iter().find(|(k, _)| *k == kind).map(|(k, _)| *k));
    let selected = match (kind, row.get::<_, Option<i64>>(3)?) {
        (Some(kind), Some(id)) => Some(EntityRef { kind, id: Id(id) }),
        _ => None,
    };
    Ok(RecentSearch { id: Id(row.get(0)?), query: row.get(1)?, selected, searched_at: row.get(4)? })
}

/// `user`'s recent searches in `library` made at `since` (milliseconds) or later, newest first,
/// at most [`KEPT`]. The server passes the start of the window it keeps them for; a device, which
/// has no clock of its own here, passes `i64::MIN` and leaves the window to the feed's deletions.
pub fn list(
    conn: &Connection,
    user: i64,
    library: i64,
    since: i64,
) -> rusqlite::Result<RecentSearches> {
    let items = conn
        .prepare_cached(&format!(
            "SELECT {COLUMNS} WHERE user_id = ?1 AND library_id = ?2 AND searched_at >= ?3 \
             ORDER BY searched_at DESC, id DESC LIMIT ?4"
        ))?
        .query_map(params![user, library, since, KEPT], row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(RecentSearches { items })
}

/// The recent search `id`, whoever's it is: the caller names one it just recorded.
pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<RecentSearch> {
    conn.prepare_cached(&format!("SELECT {COLUMNS} WHERE id = ?1"))?.query_row([id], row)
}
