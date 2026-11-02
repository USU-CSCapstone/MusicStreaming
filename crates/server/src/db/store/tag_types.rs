//! The tag types plugins added, which a scan reads with the built-in ones, and the generation of
//! tag types it reads them with (`design/database.md` §3, `design/scanning.md` §4).

use jewelcase_scanner::{AddedTagType, BUILT_IN_GENERATION, TagTypes};
use rusqlite::Connection;

use super::SqliteStore;
use crate::db::DbError;

/// The tag types a scan reads, plugins' oldest first, with the generation they are read with.
/// The generation is read first: a type added between the two reads then has files read again
/// for it, where the other way round they could take the newer generation without its field.
pub fn read(store: &SqliteStore) -> Result<TagTypes, DbError> {
    store.db.read_blocking(|conn| {
        let generation = generation(conn)?;
        let added = conn
            .prepare_cached(
                "SELECT key, field FROM tag_types WHERE plugin_id IS NOT NULL ORDER BY since, key",
            )?
            .query_map([], |row| Ok(AddedTagType { key: row.get(0)?, field: row.get(1)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(TagTypes::with(added, generation))
    })
}

/// The generation of tag types a scan reads with.
pub fn generation(conn: &Connection) -> rusqlite::Result<u32> {
    conn.prepare_cached("SELECT generation FROM tag_generation")?.query_row([], |row| row.get(0))
}

/// Raises the generation by one, for a change to what a scan reads.
pub fn raise(conn: &Connection) -> rusqlite::Result<()> {
    conn.prepare_cached("UPDATE tag_generation SET generation = generation + 1")?.execute([])?;
    Ok(())
}

/// Counts a release that adds to the built-in types ([`BUILT_IN_GENERATION`]) as a change to
/// what a scan reads, once, as the server starts. An older release leaves the generation as it
/// is, since it never falls.
pub fn count_built_in(conn: &Connection) -> rusqlite::Result<()> {
    let counted: u32 =
        conn.query_row("SELECT built_in FROM tag_generation", [], |row| row.get(0))?;
    if counted < BUILT_IN_GENERATION {
        conn.execute(
            "UPDATE tag_generation SET generation = generation + 1, built_in = ?1",
            [BUILT_IN_GENERATION],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing::open;

    /// A new database counts the built-in types as it opens, and a release adding one raises the
    /// generation once, however often it starts.
    #[tokio::test]
    async fn a_release_adding_a_built_in_type_raises_the_generation_once() {
        let (_temp, db) = open();
        let row = || {
            db.read(|conn| {
                conn.query_row("SELECT generation, built_in FROM tag_generation", [], |r| {
                    Ok((r.get::<_, u32>(0)?, r.get::<_, u32>(1)?))
                })
            })
        };
        assert_eq!(row().await.unwrap(), (1, BUILT_IN_GENERATION));

        // As the release before this one left it.
        db.write(|tx| tx.execute("UPDATE tag_generation SET built_in = built_in - 1", []))
            .await
            .unwrap();
        for _ in 0..2 {
            db.write(|tx| count_built_in(tx)).await.unwrap();
        }
        assert_eq!(row().await.unwrap(), (2, BUILT_IN_GENERATION));
    }
}
