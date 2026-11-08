//! The catalog snapshot: a library's device rows copied out of the server's database into a
//! device-schema file (`design/general.md` §3), which a device downloads instead of replaying the
//! feed.

use rusqlite::{Connection, params};

/// The device tables of one account's own rows, which the account feed carries and a snapshot
/// shared by every account that reaches the library never does.
const ACCOUNT_TABLES: &str = "'playlists', 'playlist_items'";

/// Copies `library`'s rows into `conn`'s own, empty, device-schema database from the server's
/// database, which the caller attached as `server` beforehand (SQLite cannot attach inside a
/// transaction). Only the device schema's tables and columns are read, so nothing server-only
/// can reach the copy, and none of the account tables; a tag type is copied only if one of the
/// library's tags has it.
///
/// Returns the library's feed position. It and the rows are read in one transaction, so the
/// copy is exactly the library as of that position. The schema's indexes are dropped for the
/// copy and created again once it is committed, which is far cheaper than keeping them up to
/// date row by row, and ends the read of the server's database sooner.
pub fn copy(conn: &mut Connection, library: i64) -> rusqlite::Result<i64> {
    let tx = conn.transaction()?;
    let indexes: Vec<(String, String)> = tx
        .prepare("SELECT name, sql FROM main.sqlite_schema WHERE type = 'index' AND sql NOT NULL")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (name, _) in &indexes {
        tx.execute_batch(&format!("DROP INDEX main.{name}"))?;
    }
    // Read first: this opens the server database's snapshot, which every copy below shares.
    let seq = tx.query_row(
        "SELECT coalesce(max(seq), 0) FROM server.library_changes WHERE library_id = ?1",
        [library],
        |row| row.get(0),
    )?;
    let tables: Vec<(String, String)> = tx
        .prepare(&format!(
            "SELECT m.name, group_concat(p.name, ', ') FROM sqlite_schema AS m, \
             pragma_table_info(m.name) AS p WHERE m.type = 'table' \
             AND m.name NOT IN ({ACCOUNT_TABLES}) GROUP BY m.name"
        ))?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (table, columns) in tables {
        let scope = match table.as_str() {
            "tag_types" => "key IN (SELECT type FROM server.tags WHERE library_id = ?1)",
            "libraries" => "id = ?1",
            _ => "library_id = ?1",
        };
        tx.execute(
            &format!(
                "INSERT INTO main.{table} ({columns}) SELECT {columns} FROM server.{table} \
                 WHERE {scope}"
            ),
            params![library],
        )?;
    }
    tx.commit()?;
    let tx = conn.transaction()?;
    for (_, sql) in indexes {
        tx.execute_batch(&sql)?;
    }
    tx.commit()?;
    Ok(seq)
}
