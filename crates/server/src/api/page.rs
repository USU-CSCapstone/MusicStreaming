//! What every browse list shares: its parameters, its response, and its total.
//! The queries themselves are keyset paging (`keyset`) over the rows a filter picks (`filter`).

mod filter;
mod keyset;

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

pub use filter::{Check, Lead, Table};
#[cfg(test)]
pub use keyset::assert_indexed;
pub use keyset::{Sort, Source, Total, Unknown};

use super::Problem;

pub const DEFAULT_LIMIT: u32 = 100;
pub const MAX_LIMIT: u32 = 1000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    #[default]
    Asc,
    Desc,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub total: i64,
}

/// A list's parameters, checked before the query runs.
pub struct Request {
    sort: &'static Sort,
    order: Order,
    /// The sort values the cursor continues after.
    after: Option<Vec<Value>>,
    limit: usize,
}

impl Request {
    pub fn new(
        sort: &'static Sort,
        order: Order,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> Result<Request, Problem> {
        let after = sort.after(cursor.as_deref(), order)?;
        let limit = self::limit(limit)?;
        Ok(Request { sort, order, after, limit })
    }

    /// Reads the page of `source`, with its total as `source` says.
    pub fn read<T>(
        self,
        conn: &Connection,
        source: &Source,
        map: impl FnMut(&Row) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<Page<T>> {
        let total = match source.total {
            Total::Stored(total) => total,
            Total::Counted => keyset::count(conn, source)?,
        };
        let (items, next_cursor) = keyset::fetch(conn, source, &self, map)?;
        Ok(Page { items, next_cursor, total })
    }
}

/// Checks `limit`, which the spec bounds from 1 to 1000.
pub fn limit(limit: Option<u32>) -> Result<usize, Problem> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if (1..=MAX_LIMIT).contains(&limit) {
        Ok(limit as usize)
    } else {
        Err(Problem::invalid("limit must be from 1 to 1000"))
    }
}

/// The library's stored number in `column`, such as `album_count` or `names_version`, which the
/// scanner keeps current; 0 once the library is gone, as when it was deleted after the request
/// was let in.
pub fn library_count(
    conn: &Connection,
    library: i64,
    column: &'static str,
) -> rusqlite::Result<i64> {
    let count = conn
        .prepare_cached(&format!("SELECT {column} FROM libraries WHERE id = ?1"))?
        .query_row([library], |row| row.get(0))
        .optional()?;
    Ok(count.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::library_count;
    use crate::api::testing::app_with_tracks;

    #[tokio::test]
    async fn a_library_that_is_gone_counts_nothing() {
        let (_temp, db, _app) = app_with_tracks().await;
        let count = |library| db.read(move |conn| library_count(conn, library, "track_count"));
        assert!(count(1).await.unwrap() > 0);
        assert_eq!(count(999).await.unwrap(), 0);
    }
}
