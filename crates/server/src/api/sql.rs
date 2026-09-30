//! SQL the responses share for reading rows as the API shows them.

use rusqlite::{Connection, Row};

/// SQL for a millisecond timestamp column as an RFC 3339 string, with the milliseconds computed
/// in integer arithmetic so they come out exact.
macro_rules! timestamp {
    ($column:literal) => {
        concat!(
            "strftime('%Y-%m-%dT%H:%M:%S', ",
            $column,
            " / 1000, 'unixepoch') || printf('.%03dZ', ",
            $column,
            " % 1000)"
        )
    };
}
pub(crate) use timestamp;

/// The rows of `table`, named with its alias such as `albums al`, with these IDs in this
/// library, in the order of `ids`, mapped by `map` from the columns in `select`. An ID that is
/// not there is left out.
pub fn by_ids<T>(
    conn: &Connection,
    select: &[&str],
    table: &str,
    library: i64,
    ids: &[i64],
    mut map: impl FnMut(&Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    let alias = table
        .rsplit(' ')
        .next()
        .expect("split yields at least one piece");
    // The IDs lead the join, so the rows come out in their order.
    let sql = format!(
        "SELECT {} FROM json_each(?2) AS ids CROSS JOIN {table} ON {alias}.id = ids.value \
         WHERE {alias}.library_id = ?1 ORDER BY ids.key",
        select.join(", ")
    );
    let ids = serde_json::to_string(ids).expect("integers serialize");
    conn.prepare_cached(&sql)?
        .query_map(rusqlite::params![library, ids], |row| map(row))?
        .collect()
}
