//! Schema migrations, compiled into the binary and applied at startup
//! (requirements/deployment.md §5).
//!
//! `PRAGMA user_version` records how many have been applied. Each migration runs in one
//! transaction with its version bump, so an interrupted upgrade leaves the previous version
//! intact and the next start picks up where it stopped. Foreign keys are off while they
//! run and checked before each commit (design/database.md §7).

use anyhow::{Context, bail};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use tracing::info;

/// One migration file.
pub struct Migration {
    /// The file's name without its extension, such as `0001_libraries`.
    pub name: &'static str,
    /// The file's SQL, run as one batch.
    pub sql: &'static str,
}

/// Every file in `migrations/`, in order, listed by `build.rs`.
pub const MIGRATIONS: &[Migration] = include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

/// Applies every migration the database has not seen yet, and returns the schema version.
///
/// Foreign keys are off while migrations run, so rebuilding a parent table doesn't cascade
/// into its children, and a migration commits only if every reference still resolves. The
/// connection's own setting is restored afterwards, whether or not the migrations succeed.
pub fn migrate(conn: &mut Connection, migrations: &[Migration]) -> anyhow::Result<usize> {
    let applied = version(conn)?;
    if applied > migrations.len() {
        bail!(
            "the database is at schema version {applied}, but this version of Jewelcase only \
             knows {}; run the newer version, or restore a backup taken before upgrading",
            migrations.len()
        );
    }

    // SQLite ignores this pragma inside a transaction, so it is switched outside the
    // migrations' transactions.
    let enforced: bool = conn.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    conn.pragma_update(None, "foreign_keys", false)?;
    let outcome = apply(conn, migrations, applied);
    let restored = conn.pragma_update(None, "foreign_keys", enforced);
    outcome?;
    restored?;
    Ok(migrations.len())
}

/// Applies `migrations` after the first `applied`, each in one transaction with its version
/// bump. A migration that leaves a broken reference fails and is rolled back.
fn apply(conn: &mut Connection, migrations: &[Migration], applied: usize) -> anyhow::Result<()> {
    for (index, migration) in migrations.iter().enumerate().skip(applied) {
        let failed = || format!("migration {} failed", migration.name);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(migration.sql).with_context(failed)?;
        let broken: Option<(String, String)> = tx
            .query_row("PRAGMA foreign_key_check", [], |row| Ok((row.get(0)?, row.get(2)?)))
            .optional()
            .with_context(failed)?;
        if let Some((table, parent)) = broken {
            bail!(
                "after migration {}, rows in {table} refer to missing rows in {parent}",
                migration.name
            );
        }
        tx.pragma_update(None, "user_version", index + 1)?;
        tx.commit()?;
        info!(migration = migration.name, "applied migration");
    }
    Ok(())
}

/// How many migrations the database has had, as its `user_version` records.
fn version(conn: &Connection) -> rusqlite::Result<usize> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether the database has a table named `name`.
    fn table_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            == 1
    }

    /// An in-memory connection configured as the server configures its own.
    fn configured() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::configure(&conn).unwrap();
        conn
    }

    /// Whether the connection enforces foreign keys.
    fn foreign_keys(conn: &Connection) -> bool {
        conn.pragma_query_value(None, "foreign_keys", |row| row.get(0)).unwrap()
    }

    /// The number of rows in `table`.
    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).unwrap()
    }

    /// A parent table and a child whose rows are deleted with their parent.
    const PARENT_AND_CHILD: Migration = Migration {
        name: "0001_parent_and_child",
        sql: "CREATE TABLE parent (id INTEGER PRIMARY KEY);
              CREATE TABLE child (parent_id INTEGER NOT NULL REFERENCES parent ON DELETE CASCADE);
              INSERT INTO parent VALUES (1);
              INSERT INTO child VALUES (1);",
    };

    /// The usual way to change a table's constraints: copy it into a new table, drop the old.
    const REBUILD_PARENT: Migration = Migration {
        name: "0002_rebuild_parent",
        sql: "CREATE TABLE parent_new (id INTEGER PRIMARY KEY, name TEXT);
              INSERT INTO parent_new (id) SELECT id FROM parent;
              DROP TABLE parent;
              ALTER TABLE parent_new RENAME TO parent;",
    };

    /// The tables that foreign keys refer to but that don't exist, as `child -> parent`.
    fn missing_parents(conn: &Connection) -> Vec<String> {
        conn.prepare(
            "SELECT DISTINCT t.name || ' -> ' || f.\"table\"
             FROM sqlite_schema AS t, pragma_foreign_key_list(t.name) AS f
             WHERE t.type = 'table'
               AND f.\"table\" COLLATE NOCASE
                   NOT IN (SELECT name FROM sqlite_schema WHERE type = 'table')",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    }

    #[test]
    fn applies_everything_to_a_new_database_once() {
        let mut conn = configured();
        assert!(foreign_keys(&conn), "migrates the way the server does, with foreign keys on");
        assert_eq!(migrate(&mut conn, MIGRATIONS).unwrap(), MIGRATIONS.len());
        assert_eq!(version(&conn).unwrap(), MIGRATIONS.len());
        assert!(foreign_keys(&conn), "foreign keys are back on");
        assert!(table_exists(&conn, "plugin_cursors"), "the last migration ran");

        // On empty tables, foreign_key_check misses a reference to a table that doesn't exist,
        // so that is looked for directly. It does fail on a reference to columns that aren't a key.
        assert_eq!(missing_parents(&conn), Vec::<String>::new());
        let broken = conn.prepare("PRAGMA foreign_key_check").unwrap().exists([]).unwrap();
        assert!(!broken, "every reference resolves");
        let integrity: String =
            conn.query_row("PRAGMA integrity_check", [], |row| row.get(0)).unwrap();
        assert_eq!(integrity, "ok");

        // A second run finds nothing to do.
        assert_eq!(migrate(&mut conn, MIGRATIONS).unwrap(), MIGRATIONS.len());
    }

    #[test]
    fn a_failed_migration_leaves_the_previous_version_intact() {
        let migrations = [
            Migration { name: "0001_good", sql: "CREATE TABLE a (x INTEGER);" },
            Migration {
                name: "0002_bad",
                sql: "CREATE TABLE b (x INTEGER); SELECT * FROM missing;",
            },
        ];
        let mut conn = Connection::open_in_memory().unwrap();
        let error = migrate(&mut conn, &migrations).unwrap_err();
        assert!(format!("{error:#}").contains("0002_bad"), "{error:#}");
        assert_eq!(version(&conn).unwrap(), 1);
        assert!(table_exists(&conn, "a"));
        assert!(!table_exists(&conn, "b"));
    }

    #[test]
    fn rebuilding_a_parent_table_keeps_its_children() {
        let mut conn = configured();
        migrate(&mut conn, &[PARENT_AND_CHILD, REBUILD_PARENT]).unwrap();
        assert_eq!(count(&conn, "child"), 1);
        assert!(foreign_keys(&conn), "foreign keys are back on");
    }

    #[test]
    fn a_migration_that_breaks_a_reference_is_rolled_back() {
        let orphan = Migration { name: "0002_orphan", sql: "DELETE FROM parent;" };
        let mut conn = configured();
        let error = migrate(&mut conn, &[PARENT_AND_CHILD, orphan]).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains(
                "after migration 0002_orphan, rows in child refer to missing rows in parent"
            ),
            "{message}"
        );
        assert_eq!(version(&conn).unwrap(), 1);
        assert_eq!(count(&conn, "parent"), 1);
        assert!(foreign_keys(&conn), "foreign keys are back on");
    }

    #[test]
    fn leaves_foreign_keys_off_for_a_caller_that_had_them_off() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", false).unwrap();
        migrate(&mut conn, &[PARENT_AND_CHILD]).unwrap();
        assert!(!foreign_keys(&conn));
    }

    /// Adds library `id` and one root as a database at an earlier migration holds them: their
    /// rows alone, since creating a library today also makes its users' Liked Songs and records
    /// them in feeds that may not exist yet.
    fn older_library(conn: &Connection, id: i64) {
        conn.execute_batch(&format!(
            "INSERT INTO libraries (id, name, created_at) VALUES ({id}, 'Music', 0);
             INSERT INTO library_roots (id, library_id, path, created_at)
             VALUES ({id}, {id}, '/music/{id}', 0);"
        ))
        .unwrap();
    }

    #[test]
    fn the_account_feed_starts_with_the_playlists_that_existed_numbered_per_account() {
        use crate::db::testing::{Album, Track, User};

        let mut conn = configured();
        let before = MIGRATIONS.iter().position(|m| m.name.starts_with("0011")).unwrap();
        migrate(&mut conn, &MIGRATIONS[..before]).unwrap();

        User::new(1, "owner").insert(&conn).unwrap();
        User::new(2, "user").insert(&conn).unwrap();
        older_library(&conn, 1);
        Album::new(1, 1).insert(&conn).unwrap();
        Track::new(1, 1, 1).insert(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, created_at, \
                                    modified_at)
             VALUES (7, 1, 1, 'Old', 'old', x'', 0, 0), (8, 1, 2, 'Theirs', 'theirs', x'', 0, 0);
             INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at)
             VALUES (7, 1048576, 1, 1, 0), (7, 2097152, 1, 1, 0);",
        )
        .unwrap();

        migrate(&mut conn, MIGRATIONS).unwrap();

        type Row = (i64, i64, String, i64, i64, String, i64);
        let feed: Vec<Row> = conn
            .prepare(
                "SELECT user_id, library_id, kind, entity_id, item_key, op, position \
                 FROM account_changes ORDER BY seq",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let row = |user, kind: &str, id, key, position| {
            (user, 1, kind.to_owned(), id, key, "upsert".to_owned(), position)
        };
        assert_eq!(
            feed,
            [
                row(1, "playlist", 7, 0, 1),
                row(2, "playlist", 8, 0, 1),
                row(1, "playlistItem", 7, 1_048_576, 2),
                row(1, "playlistItem", 7, 2_097_152, 3),
            ]
        );
        let heads: Vec<(i64, i64)> = conn
            .prepare("SELECT id, feed_head FROM users ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(heads, [(1, 3), (2, 1)]);
    }

    #[test]
    fn existing_changes_are_numbered_per_library() {
        let mut conn = configured();
        let before = MIGRATIONS.iter().position(|m| m.name.starts_with("0013")).unwrap();
        migrate(&mut conn, &MIGRATIONS[..before]).unwrap();
        for id in [1, 2] {
            older_library(&conn, id);
        }
        conn.execute_batch(
            "INSERT INTO library_changes (library_id, entity_type, entity_id, op, changed_at)
             VALUES (1, 'track', 1, 'upsert', 0), (2, 'track', 2, 'upsert', 0),
                    (2, 'track', 3, 'upsert', 0), (1, 'track', 4, 'upsert', 0);",
        )
        .unwrap();

        migrate(&mut conn, MIGRATIONS).unwrap();

        let rows = |sql: &str| -> Vec<(i64, i64)> {
            conn.prepare(sql)
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let library = "SELECT library_id, position FROM library_changes ORDER BY seq";
        assert_eq!(rows(library), [(1, 1), (2, 1), (2, 2), (1, 2)]);
        assert_eq!(rows("SELECT id, feed_head FROM libraries ORDER BY id"), [(1, 2), (2, 2)]);
    }

    #[test]
    fn recent_searches_kept_before_join_the_account_feed_after_each_head() {
        use crate::db::testing::User;

        let mut conn = configured();
        let before = MIGRATIONS.iter().position(|m| m.name.starts_with("0014")).unwrap();
        migrate(&mut conn, &MIGRATIONS[..before]).unwrap();
        User::new(1, "owner").insert(&conn).unwrap();
        User::new(2, "user").insert(&conn).unwrap();
        older_library(&conn, 1);
        conn.execute_batch(
            "INSERT INTO account_changes (user_id, library_id, kind, entity_id, op, position)
             VALUES (1, 1, 'hiddenTrack', 5, 'upsert', 1);
             UPDATE users SET feed_head = 1 WHERE id = 1;
             INSERT INTO recent_searches (id, user_id, library_id, query, folded, searched_at)
             VALUES (7, 1, 1, 'Later', 'later', 20), (8, 1, 1, 'Sooner', 'sooner', 10),
                    (9, 2, 1, 'Theirs', 'theirs', 30);",
        )
        .unwrap();

        migrate(&mut conn, MIGRATIONS).unwrap();

        type Row = (i64, String, i64, i64);
        let feed: Vec<Row> = conn
            .prepare("SELECT user_id, kind, entity_id, position FROM account_changes ORDER BY seq")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let row = |user, kind: &str, id, position| (user, kind.to_owned(), id, position);
        assert_eq!(
            feed,
            [
                row(1, "hiddenTrack", 5, 1),
                row(1, "recentSearch", 8, 2),
                row(1, "recentSearch", 7, 3),
                row(2, "recentSearch", 9, 1),
            ]
        );
        let heads: Vec<(i64, i64)> = conn
            .prepare("SELECT id, feed_head FROM users ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(heads, [(1, 3), (2, 1)]);
    }

    #[test]
    fn answers_stored_before_content_types_were_kept_replay_as_json() {
        use crate::db::testing::User;

        let mut conn = configured();
        let before = MIGRATIONS.iter().position(|m| m.name.starts_with("0012")).unwrap();
        migrate(&mut conn, &MIGRATIONS[..before]).unwrap();
        User::new(1, "owner").insert(&conn).unwrap();
        conn.execute(
            "INSERT INTO idempotency_keys (user_id, key, status, body, created_at) \
             VALUES (1, 'k', 201, '{}', 0)",
            [],
        )
        .unwrap();

        migrate(&mut conn, MIGRATIONS).unwrap();

        let content_type: String = conn
            .query_row("SELECT content_type FROM idempotency_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(content_type, "application/json");
    }

    #[test]
    fn refuses_a_database_from_a_newer_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        let error = migrate(&mut conn, MIGRATIONS).unwrap_err();
        assert!(error.to_string().contains("schema version 99"), "{error:#}");
    }
}
