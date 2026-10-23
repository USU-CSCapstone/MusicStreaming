//! Reading a library's rows by ID, in the order the IDs were asked for.

use rusqlite::{Connection, Row, params};
use serde::Serialize;

use super::Json;

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

#[cfg(test)]
mod tests {
    use super::by_ids;
    use crate::db::testing::{Album, library, open};

    /// Reads the IDs of albums `ids` in library 1 that meet `rest`.
    fn album_ids(rest: &'static str, ids: &[i64]) -> Vec<i64> {
        let (_temp, db) = open();
        db.write_blocking(|tx| {
            library(tx, 1, "Music", "/music")?;
            library(tx, 2, "Other", "/other")?;
            Album::new(101, 1).insert(tx)?;
            Album { track_count: 1, ..Album::new(102, 1) }.insert(tx)?;
            Album::new(201, 2).insert(tx)
        })
        .unwrap();
        let ids = ids.to_vec();
        db.read_blocking(move |conn| {
            by_ids(conn, &["al.id"], "albums al", rest, 1, &ids, |row| row.get(0))
        })
        .unwrap()
    }

    #[test]
    fn by_ids_reads_only_the_librarys_rows_in_the_order_asked() {
        assert_eq!(album_ids("", &[102, 201, 999, 101]), [102, 101]);
    }

    #[test]
    fn by_ids_leaves_out_rows_that_fail_the_where_clause() {
        assert_eq!(album_ids("WHERE al.track_count > 0", &[102, 201, 101]), [102]);
    }
}
