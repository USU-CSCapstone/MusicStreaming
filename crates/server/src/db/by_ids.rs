//! Reading a library's rows by ID, in the order the IDs were asked for
//! (`jewelcase_catalog::by_ids`).

pub use jewelcase_catalog::by_ids::by_ids;

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
