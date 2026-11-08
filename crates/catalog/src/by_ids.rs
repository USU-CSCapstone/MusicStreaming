//! Reading a library's rows by ID, in the order the IDs were asked for.

use rusqlite::{Connection, Row, params};
use serde::Serialize;

use crate::Json;

/// The rows of `table`, named with its alias such as `albums al`, with these IDs in this
/// library, in the order of `ids`, mapped by `map` from the columns in `select`. `rest` follows
/// the join that finds them, which checks the library: further joins, which `select` may read,
/// then a `WHERE` clause the rows must meet too, if any. An ID that is not there, or whose row
/// fails `rest`, is left out. The SQL is static text, so nothing from outside reaches it.
pub fn by_ids<T>(
    conn: &Connection,
    select: &[&'static str],
    table: &'static str,
    rest: &'static str,
    library: i64,
    ids: &[impl Serialize],
    mut map: impl FnMut(&Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    let alias = table.rsplit_once(' ').map_or(table, |(_, alias)| alias);
    // The IDs lead the join, so the rows come out in their order.
    let sql = format!(
        "SELECT {} FROM json_each(?2) AS ids CROSS JOIN {table} \
         ON {alias}.id = ids.value AND {alias}.library_id = ?1 {rest} ORDER BY ids.key",
        select.join(", ")
    );
    conn.prepare_cached(&sql)?.query_map(params![library, Json(ids)], |row| map(row))?.collect()
}
