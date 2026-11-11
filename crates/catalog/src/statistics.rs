//! Query-planner statistics, gathered by one rule wherever a catalog lives: the server's
//! database, the snapshot a device downloads, and the device's copy as its feed grows it
//! (`design/database.md` §4).

use rusqlite::Connection;

/// The fewest tracks, across every library a database holds, for which statistics are gathered.
/// Statistics from a smaller library describe tables of a few rows, which lead the planner to
/// scan a table that has since grown rather than look its rows up, until `PRAGMA optimize`
/// notices that it has grown tenfold. Without statistics the planner takes every table to be
/// large.
pub const STATISTICS_FROM_TRACKS: i64 = 1_000;

/// Gathers statistics for every table that lacks them or has grown or shrunk tenfold since they
/// were gathered (0x10000 checks every table, 0x2 analyzes), within SQLite's analysis limit
/// (0x10), so even a large database is quick to analyze. When no table needs it, it only reads
/// each table's size.
pub const OPTIMIZE: &str = "PRAGMA optimize = 0x10012";

/// Runs [`OPTIMIZE`] when the libraries together hold at least [`STATISTICS_FROM_TRACKS`]
/// tracks, and does nothing otherwise.
pub fn gather(conn: &Connection) -> rusqlite::Result<()> {
    let tracks: i64 =
        conn.query_row("SELECT ifnull(SUM(track_count), 0) FROM libraries", [], |r| r.get(0))?;
    if tracks >= STATISTICS_FROM_TRACKS {
        conn.execute_batch(OPTIMIZE)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::{STATISTICS_FROM_TRACKS, gather};
    use crate::create_schema;

    /// The rows the statistics take the tracks table to hold, or 0 where none describe it.
    fn described(conn: &Connection) -> i64 {
        let gathered: bool = conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE name = 'sqlite_stat1')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if !gathered {
            return 0;
        }
        // Each row's `stat` starts with the table's row count.
        conn.query_row(
            "SELECT ifnull(MAX(CAST(stat AS INTEGER)), 0) FROM sqlite_stat1 WHERE tbl = 'tracks'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Adds tracks `from` up to `to` to library 1, and its count, as the feed keeps it.
    fn grow(conn: &Connection, from: i64, to: i64) {
        conn.execute_batch(&format!(
            "WITH RECURSIVE n (i) AS (SELECT {from} UNION ALL SELECT i + 1 FROM n WHERE i < {to} - 1) \
             INSERT INTO tracks (id, library_id, album_id, file_size, title, sort_key, \
                 artist_sort_key, album_sort_key, codec, container, lossless, sample_rate_hz, \
                 channels, duration_us, added_at) \
             SELECT i, 1, 1 + i % 100, 1, 'Track ' || i, CAST(printf('track %06d', i) AS BLOB), \
                 x'', x'', 'flac', 'flac', 1, 44100, 2, 1000, i FROM n; \
             UPDATE libraries SET track_count = {to} - 1 WHERE id = 1"
        ))
        .unwrap();
    }

    /// None for a library of a few tracks, which would describe its tables as a few rows once
    /// they have grown; gathered once it reaches real size, and again once it grows tenfold.
    #[test]
    fn statistics_wait_for_a_library_of_real_size_and_follow_it_as_it_grows() {
        let conn = Connection::open_in_memory().unwrap();
        create_schema(&conn).unwrap();
        conn.execute("INSERT INTO libraries (id) VALUES (1)", []).unwrap();
        grow(&conn, 1, 51);
        gather(&conn).unwrap();
        assert_eq!(described(&conn), 0, "50 tracks");

        grow(&conn, 51, STATISTICS_FROM_TRACKS + 1);
        gather(&conn).unwrap();
        let first = described(&conn);
        assert!(first > 0, "{STATISTICS_FROM_TRACKS} tracks");

        grow(&conn, STATISTICS_FROM_TRACKS + 1, STATISTICS_FROM_TRACKS * 11 + 1);
        gather(&conn).unwrap();
        assert!(described(&conn) > first * 5, "a tenfold growth is described again");
    }
}
