//! What every browse list shares: its parameters, its response, and its total.
//! The queries themselves are keyset paging (`keyset`).

mod keyset;

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[cfg(test)]
pub use keyset::assert_indexed;
pub use keyset::{Sort, Source, Unknown, count, fetch};

use super::{Code, Problem};

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

/// Checks `limit`, which the spec bounds from 1 to 1000.
pub fn limit(limit: Option<u32>) -> Result<usize, Problem> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if (1..=MAX_LIMIT).contains(&limit) {
        Ok(limit as usize)
    } else {
        Err(Problem::new(Code::ValidationFailed).detail("limit must be from 1 to 1000"))
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
