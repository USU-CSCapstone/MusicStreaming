//! What every browse list shares: its parameters, its response, and its total.
//! The queries themselves are keyset paging (`keyset`).

mod keyset;

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

#[cfg(test)]
pub use keyset::assert_indexed;
pub use keyset::{Sort, Source, Unknown};

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
        Ok(Request {
            sort,
            order,
            after,
            limit,
        })
    }

    /// Reads the page of `source` in `library`, or `None` if there is no such library. Its
    /// total is the library's `stored` count, such as `album_count`, unless it is `filtered`.
    pub fn read<T>(
        self,
        conn: &Connection,
        library: i64,
        stored: &'static str,
        filtered: bool,
        source: &Source,
        map: impl FnMut(&Row) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<Option<Page<T>>> {
        let Some(mut total) = library_count(conn, library, stored)? else {
            return Ok(None);
        };
        if filtered {
            total = keyset::count(conn, source)?;
        }
        let (items, next_cursor) = keyset::fetch(conn, source, &self, map)?;
        Ok(Some(Page {
            items,
            next_cursor,
            total,
        }))
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

/// The library's stored count in `column`, such as `album_count`, which the scanner keeps
/// current; `None` if there is no such library.
pub fn library_count(
    conn: &Connection,
    library: i64,
    column: &'static str,
) -> rusqlite::Result<Option<i64>> {
    conn.prepare_cached(&format!("SELECT {column} FROM libraries WHERE id = ?1"))?
        .query_row([library], |row| row.get(0))
        .optional()
}
