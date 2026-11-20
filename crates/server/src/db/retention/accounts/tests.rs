//! Emptying an account being deleted: every row of it goes, one chunk of one table per write,
//! through an index, and its `users` row last; nothing of anyone else's changes, a live account
//! is never touched, and a run cut short is finished by the next sweep.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use jewelcase_plugins::{Hook, Permission};
use rusqlite::types::Value;
use rusqlite::{Connection, Result, Transaction, params, params_from_iter};

use super::{DELETING, LAST, TABLES, empty, step};
use crate::db::testing::{self, Album, Device, Track, User, access, library, mark_deleting};
use crate::db::{Database, feed, liked_songs};

/// The account being deleted.
const GONE: i64 = 2;
/// Another account, with the same kinds of rows.
const OTHER: i64 = 3;
/// When the rows were written, and when the sweep runs.
const NOW: i64 = 1_000 * 24 * 60 * 60 * 1000;
/// How many rows each account has of each kind that can have more than a chunk.
const MANY: i64 = 250;
/// How many tracks each queue list holds: 400 KB each.
const LIST_LEN: i64 = 50_000;

/// The tables left to the final delete's cascade, each holding a handful of the account's rows:
/// its one player state; a grant and a deleted Liked Songs per library at most; a connection per
/// plugin at most; and the invites it made or redeemed, whose links are set to `NULL` through
/// `invites_by_creator` and `invites_by_redeemer`, and which the sweep deletes 30 days after they
/// end.
const CASCADE: [&str; 5] =
    ["player_states", "library_access", "liked_songs_deleted", "plugin_connections", "invites"];

/// The tables whose rows go with their parents, which the task deletes: a play's events and a
/// queue's uses of lists.
const WITH_PARENTS: [&str; 2] = ["play_events", "queue_list_uses"];

/// Adds play `?1` of account `?2` on device `?3`, which records its end in `play_events`.
const PLAY: &str = "INSERT INTO plays (id, user_id, device_id, library_id, track_id, title, \
     artists, artist_keys, album_title_key, album_artists_key, genre_keys, duration_us, \
     started_at, listen_time_ms, ended, context, origin, updated_at) \
     VALUES (?1, ?2, ?3, 1, 1, 'T', '[]', '[]', '', '', '[]', 1000, ?4, 0, 'finished', '{}', \
             'manual', ?4)";

/// Saves account `?1`'s player state, as `Players::save_where` does.
const PLAYER_STATE: &str = "INSERT OR REPLACE INTO player_states (user_id, item_id, position_ms) \
     VALUES (?1, ?2, 0)";

/// A database with libraries 1 and 2, the owner, five tracks, a plugin, an anonymous sign-in
/// attempt, and accounts [`GONE`] and [`OTHER`] with every kind of row.
fn open() -> (tempfile::TempDir, Arc<Database>) {
    let (temp, db) = testing::open();
    db.write_blocking(|tx| {
        library(tx, 1, "One", "/one")?;
        library(tx, 2, "Two", "/two")?;
        User::new(1, "owner").insert(tx)?;
        Album::new(10, 1).insert(tx)?;
        for id in 1..=5 {
            Track::new(id, 1, 10).insert(tx)?;
        }
        tx.execute("INSERT INTO plugins (id, manifest, installed_at) VALUES ('p', '{}', 0)", [])?;
        for started in 100..100 + MANY {
            tx.execute("INSERT INTO feed_epochs (started, epoch) VALUES (?1, ?1)", [started])?;
        }
        tx.execute(
            "INSERT INTO login_attempts (attempted_at, username_attempted, origin, succeeded) \
             VALUES (?1, 'nobody', '192.0.2.9', 0)",
            [NOW],
        )?;
        populate(tx, GONE)?;
        populate(tx, OTHER)
    })
    .unwrap();
    (temp, db)
}

/// Adds account `user` with rows of every kind: plays that each name one of its devices, with
/// their events; playlists, one of them large, and a Liked Songs; account changes, hidden tracks,
/// mark and search events, recent searches; a live and a kept queue using three large lists;
/// idempotency keys, feed starts, sign-in attempts named `user<id>`; a cursor in each library for
/// every hook that keeps one per user; a connection, a player state, a grant, a deleted Liked
/// Songs; and an invite it made and redeemed, made at its ID so a test can tell it apart.
fn populate(conn: &Connection, user: i64) -> Result<()> {
    let base = user * 1000;
    User::new(user, "user").insert(conn)?;
    access(conn, user, 1)?;
    for id in base..base + 3 {
        let mut hash = [0; 32];
        hash[..8].copy_from_slice(&id.to_be_bytes());
        Device::new(user, id, hash).insert(conn)?;
    }
    let mut play = conn.prepare(PLAY)?;
    for n in 0..MANY {
        play.execute(params![n.to_string(), user, base + n % 3, NOW])?;
    }
    for (id, kind, items) in
        [(base + 1, "saved", MANY), (base + 2, "saved", 3), (base + 3, "liked", 2)]
    {
        conn.execute(
            "INSERT INTO playlists (id, library_id, user_id, kind, title, folded, sort_key, \
             created_at, modified_at) VALUES (?1, 1, ?2, ?3, 'P', 'p', x'70', 0, 0)",
            params![id, user, kind],
        )?;
        for key in 0..items {
            conn.execute(
                "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
                 VALUES (?1, ?2, 1, ?3, 0)",
                [id, key, key % 5 + 1],
            )?;
        }
    }
    for n in 0..MANY {
        conn.execute(
            "INSERT INTO account_changes (user_id, library_id, kind, entity_id, op) \
             VALUES (?1, 1, 'playlist', ?2, 'upsert')",
            [user, n],
        )?;
        conn.execute(
            "INSERT INTO mark_events (user_id, library_id, track_id, kind, created_at) \
             VALUES (?1, 1, 1, 'liked', ?2)",
            [user, NOW],
        )?;
        conn.execute(
            "INSERT INTO search_events (user_id, library_id, kind, search_id, found_tracks, \
             found_albums, found_artists, created_at) VALUES (?1, 1, 'searched', ?2, 0, 0, 0, ?3)",
            [user, n, NOW],
        )?;
        conn.execute(
            "INSERT INTO recent_searches (user_id, library_id, query, folded, searched_at) \
             VALUES (?1, 1, 'q' || ?2, 'q' || ?2, ?3)",
            [user, n, NOW],
        )?;
        conn.execute(
            "INSERT INTO idempotency_keys (user_id, key, status, body, created_at) \
             VALUES (?1, 'k' || ?2, 200, '{}', ?3)",
            [user, n, NOW],
        )?;
        conn.execute(
            "INSERT INTO account_feed_starts (started, user_id, head) VALUES (?1, ?2, 0)",
            [100 + n, user],
        )?;
        conn.execute(
            "INSERT INTO login_attempts (attempted_at, username_attempted, user_id, origin, \
             succeeded) VALUES (?1, 'user' || ?2, ?2, '192.0.2.1', 0)",
            [NOW, user],
        )?;
    }
    for track in 1..=5 {
        conn.execute(
            "INSERT INTO hidden_tracks (user_id, library_id, track_id, hidden_at) \
             VALUES (?1, 1, ?2, 0)",
            [user, track],
        )?;
    }
    queue(conn, user, "live", &["a", "b"])?;
    queue(conn, user, "kept", &["c"])?;
    for hook in personal_hooks() {
        for library in [1, 2] {
            conn.execute(
                "INSERT INTO plugin_cursors (plugin_id, library_id, hook, user_id) \
                 VALUES ('p', ?1, ?2, ?3)",
                params![library, hook, user],
            )?;
        }
    }
    conn.execute(
        "INSERT INTO plugin_connections (plugin_id, user_id, settings, connected_at) \
         VALUES ('p', ?1, '{}', 0)",
        [user],
    )?;
    conn.execute(PLAYER_STATE, params![user, "1.0"])?;
    conn.execute("INSERT INTO liked_songs_deleted (user_id, library_id) VALUES (?1, 2)", [user])?;
    conn.execute(
        "INSERT INTO invites (code_hash, created_by, redeemed_by, created_at, expires_at) \
         VALUES (?1, ?2, ?2, ?2, ?3)",
        params![user.to_be_bytes(), user, NOW],
    )?;
    Ok(())
}

/// Adds `user`'s queue `id`, live or kept by its name, and the large lists `hashes` it uses.
fn queue(conn: &Connection, user: i64, id: &str, hashes: &[&str]) -> Result<()> {
    let live = id == "live";
    conn.execute(
        "INSERT INTO queues (user_id, queue_id, version, context, state, active, last_active) \
         VALUES (?1, ?2, 0, '{}', '{}', ?3, ?4)",
        params![user, id, live, (!live).then_some(NOW)],
    )?;
    for hash in hashes {
        conn.execute(
            "INSERT INTO queue_list_uses (user_id, queue_id, hash) VALUES (?1, ?2, ?3)",
            params![user, id, hash],
        )?;
        conn.execute(
            "INSERT INTO queue_lists (user_id, hash, library_id, tracks, length) \
             VALUES (?1, ?2, 1, zeroblob(?3 * 8), ?3)",
            params![user, hash, LIST_LEN],
        )?;
    }
    Ok(())
}

/// The names of the hooks that keep a position per user.
fn personal_hooks() -> Vec<String> {
    Permission::ALL.iter().filter(|p| p.hook() == Some(Hook::Personal)).map(|p| p.name()).collect()
}

/// Every table's name.
fn tables(conn: &Connection) -> Result<Vec<String>> {
    conn.prepare(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
         ORDER BY name",
    )?
    .query_map([], |r| r.get(0))?
    .collect()
}

/// Every column that names an account: each `user_id`, and each column with a foreign key to
/// `users`, as `(table, column)`.
fn account_columns(conn: &Connection) -> Result<Vec<(String, String)>> {
    conn.prepare(
        "SELECT m.name, p.name FROM sqlite_schema m, pragma_table_info(m.name) p \
         WHERE m.type = 'table' AND p.name = 'user_id' \
         UNION SELECT m.name, f.\"from\" FROM sqlite_schema m, pragma_foreign_key_list(m.name) f \
         WHERE m.type = 'table' AND f.\"table\" = 'users' ORDER BY 1, 2",
    )?
    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
    .collect()
}

/// How many rows name account `user` in each column of [`account_columns`], as `table.column`,
/// with its own row as `users` and its playlists' items as `playlist_items`.
fn counts(db: &Database, user: i64) -> BTreeMap<String, i64> {
    db.read_blocking(move |conn| {
        let mut counts = BTreeMap::new();
        let mut count = |key: String, sql: String| -> Result<()> {
            counts.insert(key, conn.query_row(&sql, [user], |r| r.get(0))?);
            Ok(())
        };
        for (table, column) in account_columns(conn)? {
            count(
                format!("{table}.{column}"),
                format!("SELECT count(*) FROM {table} WHERE {column} = ?1"),
            )?;
        }
        count("users".into(), "SELECT count(*) FROM users WHERE id = ?1".into())?;
        count(
            "playlist_items".into(),
            "SELECT count(*) FROM playlist_items WHERE playlist_id / 1000 = ?1".into(),
        )?;
        Ok(counts)
    })
    .unwrap()
}

/// The entries of [`counts`] that are not zero.
fn left(db: &Database, user: i64) -> BTreeMap<String, i64> {
    counts(db, user).into_iter().filter(|&(_, n)| n != 0).collect()
}

/// Every row of every table, as text, but those of account `except`: its own row, the rows that
/// name it, its playlists' items, and its sign-in attempts and invites, which stay with the link
/// cleared.
fn rows(db: &Database, except: i64) -> BTreeMap<String, Vec<String>> {
    db.read_blocking(move |conn| {
        let mut all = BTreeMap::new();
        for table in tables(conn)? {
            let has_user = conn.query_row(
                "SELECT count(*) FROM pragma_table_info(?1) WHERE name = 'user_id'",
                [&table],
                |r| r.get::<_, i64>(0),
            )? > 0;
            let filter = match table.as_str() {
                "users" => "id IS NOT ?1",
                "playlist_items" => {
                    "playlist_id NOT IN (SELECT id FROM playlists WHERE user_id = ?1)"
                }
                "login_attempts" => "username_attempted IS NOT 'user' || ?1",
                "invites" => "created_at IS NOT ?1",
                _ if has_user => "user_id IS NOT ?1",
                _ => "?1 IS NOT NULL",
            };
            let mut select = conn.prepare(&format!("SELECT * FROM {table} WHERE {filter}"))?;
            let width = select.column_count();
            let mut rows = select
                .query_map([except], |row| {
                    (0..width)
                        .map(|i| row.get::<_, Value>(i).map(|value| format!("{value:?}")))
                        .collect::<Result<Vec<_>>>()
                        .map(|values| values.join("|"))
                })?
                .collect::<Result<Vec<_>>>()?;
            rows.sort();
            all.insert(table, rows);
        }
        Ok(all)
    })
    .unwrap()
}

/// Runs the task for `user` to its end.
fn run(db: &Database, user: i64) {
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(empty(db, user));
}

/// Runs one write of the task for `user`, from table `at`, and returns whether the task is over
/// and the table it goes on from.
fn write_step(db: &Database, user: i64, at: usize) -> (bool, usize) {
    db.write_blocking(move |tx| {
        let mut at = at;
        step(tx, user, &mut at).map(|done| (done, at))
    })
    .unwrap()
}

/// Runs the task for `user` up to its last write, which deletes the account's row.
fn run_to_last(db: &Database, user: i64) {
    let mut at = 0;
    while at < TABLES.len() {
        let (done, next) = write_step(db, user, at);
        assert!(!done);
        at = next;
    }
}

/// Marks account `user` as being deleted.
fn mark(db: &Database, user: i64) {
    db.write_blocking(move |tx| mark_deleting(tx, user)).unwrap();
}

/// The first column of the one row `sql` returns.
fn value(db: &Database, sql: &'static str) -> i64 {
    db.read_blocking(move |conn| conn.query_row(sql, [], |r| r.get(0))).unwrap()
}

/// Every row of the account goes, its sign-in attempts and invites stay with their link to it
/// cleared, and every other row in the database, another account's, the catalog's, the
/// libraries', is as it was.
#[test]
fn deleting_an_account_leaves_nothing_of_it_and_touches_no_one_else() {
    let (_temp, db) = open();
    mark(&db, GONE);
    let others = rows(&db, GONE);
    assert!(left(&db, GONE).len() > 20, "the account has rows in every table");

    run(&db, GONE);

    assert_eq!(left(&db, GONE), BTreeMap::new());
    assert_eq!(rows(&db, GONE), others);
    let attempts = "SELECT count(*) FROM login_attempts \
                    WHERE username_attempted = 'user2' AND user_id IS NULL";
    assert_eq!(value(&db, attempts), MANY);
    let invites = "SELECT count(*) FROM invites \
                   WHERE created_at = 2 AND coalesce(created_by, redeemed_by) IS NULL";
    assert_eq!(value(&db, invites), 1);
}

/// Each write takes at most one chunk of one table, with the rows that go with them, one queue
/// list at a time, and the account's row goes alone, in the last write, once its tables are
/// empty: it is there while any row of theirs is.
#[test]
fn each_write_takes_a_chunk_of_one_table_and_the_account_row_goes_last() {
    let (_temp, db) = open();
    mark(&db, GONE);
    let mut before = counts(&db, GONE);
    let mut at = 0;
    let mut writes = 0;
    loop {
        let (done, next) = write_step(&db, GONE, at);
        writes += 1;
        let after = counts(&db, GONE);
        let took: BTreeMap<&str, i64> = before
            .iter()
            .map(|(key, n)| (key.as_str(), n - after[key]))
            .filter(|&(_, n)| n != 0)
            .collect();
        let table_of = |key: &str| key.split('.').next().unwrap().to_owned();
        if done {
            let waiting: BTreeSet<String> =
                before.iter().filter(|&(_, &n)| n != 0).map(|(key, _)| table_of(key)).collect();
            let cascaded: BTreeSet<String> =
                CASCADE.iter().map(|&t| t.to_owned()).chain(["users".to_owned()]).collect();
            assert!(waiting.is_subset(&cascaded), "left to the last write: {waiting:?}");
            assert_eq!(after.values().sum::<i64>(), 0, "{after:?}");
            break;
        }
        assert_eq!(after["users"], 1, "the row went before {took:?}");
        let (table, _, chunk) = TABLES[at];
        let chunk = i64::try_from(chunk).unwrap();
        for (&key, &n) in &took {
            let with = match table {
                "plays" => "play_events",
                "queues" => "queue_list_uses",
                _ => table,
            };
            let of = table_of(key);
            assert!(of == table || of == with, "a write of {table} took {n} from {key}");
            assert!(n <= chunk, "a write of {table} took {n} from {key}");
            if table == "queue_lists" {
                assert_eq!(n, 1, "a write took {n} queue lists");
            }
        }
        assert_eq!(next, if took.is_empty() { at + 1 } else { at });
        before = after;
        at = next;
    }
    assert!(writes > TABLES.len() * 2, "only {writes} writes: some table went in one chunk");
}

/// A chunk that fails ends the run and leaves the account's row, with the rows of that table
/// and those after it. The next run starts again and finishes.
#[test]
fn a_failed_chunk_ends_the_run_and_the_next_finishes_it() {
    let (_temp, db) = open();
    mark(&db, GONE);
    db.write_blocking(|tx| {
        tx.execute_batch(
            "CREATE TRIGGER poison BEFORE DELETE ON plugin_cursors \
             BEGIN SELECT RAISE(ABORT, 'poison'); END;",
        )
    })
    .unwrap();
    run(&db, GONE);
    let left = left(&db, GONE);
    assert_eq!(left.get("users"), Some(&1));
    assert_eq!(left.get("plugin_cursors.user_id"), Some(&8));
    assert_eq!(left.get("plays.user_id"), None);

    db.write_blocking(|tx| tx.execute_batch("DROP TRIGGER poison")).unwrap();
    run(&db, GONE);
    assert_eq!(self::left(&db, GONE), BTreeMap::new());
}

/// The steps of `sql`'s query plan, run for [`GONE`] with `chunk`, as the task runs it, on the
/// writer.
fn plan(db: &Database, sql: &'static str, chunk: Option<usize>) -> Vec<String> {
    db.write_blocking(move |tx| {
        let mut explain = tx.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
        match chunk {
            Some(chunk) => explain.query_map(params![GONE, chunk], |r| r.get(3))?.collect(),
            None => explain.query_map([GONE], |r| r.get(3))?.collect(),
        }
    })
    .unwrap()
}

/// Replaces the statistics `ANALYZE` gathers over these few rows with those of a database of a
/// thousand times the accounts, each with the same rows. The tables of accounts' rows hold a
/// thousand times as many, and so do the values of their indexes but those that are unique or
/// name one account's rows: its ID, or its playlist's. `sqlite_stat1` holds each table's rows,
/// then, for an index, the rows per value of each of its leading parts.
fn at_scale(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("ANALYZE")?;
    let stats: Vec<(String, Option<String>, String)> = tx
        .prepare("SELECT tbl, idx, stat FROM sqlite_stat1")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_>>()?;
    for (table, index, stat) in stats {
        let columns: Vec<String> = tx
            .prepare("SELECT name FROM pragma_table_info(?1)")?
            .query_map([&table], |r| r.get(0))?
            .collect::<Result<_>>()?;
        if !["users", "playlist_items"].contains(&table.as_str())
            && !columns.iter().any(|column| column == "user_id")
        {
            continue;
        }
        let keyed: Vec<String> = tx
            .prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")?
            .query_map([&index], |r| r.get(0))?
            .collect::<Result<_>>()?;
        let mut parts = stat.split(' ');
        let rows: i64 = parts.next().unwrap().parse().unwrap();
        let mut scaled = vec![(rows * 1000).to_string()];
        for (i, part) in parts.enumerate() {
            let own = keyed.iter().take(i + 1).any(|c| c == "user_id" || c == "playlist_id");
            scaled.push(match part.parse::<i64>() {
                Ok(n) if !own && n > 1 => (n * 1000).to_string(),
                _ => part.to_owned(),
            });
        }
        tx.execute(
            "UPDATE sqlite_stat1 SET stat = ?1 WHERE tbl = ?2 AND idx IS ?3",
            params![scaled.join(" "), table, index],
        )?;
    }
    tx.execute_batch("ANALYZE sqlite_schema")
}

/// Every statement of the task finds its rows through an index, so no chunk reads a whole table,
/// `plugin_cursors`' included, whose key leads with the plugin and the library and which is
/// reached through every installed plugin and library. It holds with no statistics, as the server
/// keeps for libraries under
/// [`STATISTICS_FROM_TRACKS`](jewelcase_catalog::statistics::STATISTICS_FROM_TRACKS) tracks, and
/// with those gathered over a household's few rows, which a server keeps until its tables grow
/// tenfold, and with those of a large database ([`at_scale`]).
#[test]
fn every_statement_of_the_task_uses_an_index() {
    let (_temp, db) = open();
    for statistics in ["none", "household", "at scale"] {
        match statistics {
            "household" => db.write_blocking(|tx| tx.execute_batch("ANALYZE")).unwrap(),
            "at scale" => db.write_blocking(at_scale).unwrap(),
            _ => {}
        }
        let statements = TABLES
            .iter()
            .map(|&(table, sql, chunk)| (table, sql, Some(chunk)))
            .chain([("users", DELETING, None), ("users", LAST, None)]);
        for (table, sql, chunk) in statements {
            let plan = plan(&db, sql, chunk);
            assert!(plan.iter().any(|step| step.starts_with("SEARCH")), "{table}: {plan:?}");
            assert!(
                !plan.iter().any(|step| step.starts_with("SCAN")),
                "{table}, {statistics}: {plan:?}"
            );
        }
    }
}

/// Apart from the plans: every table that refers to `users`, or to a table the task deletes
/// from, finds the rows that refer to one through an index led by its referring columns, so no
/// delete's foreign-key action scans a table for them.
#[test]
fn the_rows_a_delete_cascades_to_are_found_through_an_index() {
    let (_temp, db) = testing::open();
    let unindexed = db
        .read_blocking(|conn| {
            let parents: Vec<&str> = TABLES.iter().map(|t| t.0).chain(["users"]).collect();
            let mut unindexed = Vec::new();
            for child in tables(conn)? {
                let mut keys: BTreeMap<i64, (String, Vec<String>)> = BTreeMap::new();
                let mut list = conn.prepare(
                    "SELECT id, \"table\", \"from\" FROM pragma_foreign_key_list(?1) \
                     ORDER BY id, seq",
                )?;
                let rows = list.query_map([&child], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                for row in rows {
                    let (id, parent, column): (i64, String, String) = row?;
                    keys.entry(id).or_insert((parent, Vec::new())).1.push(column);
                }
                for (parent, columns) in keys.into_values() {
                    if !parents.contains(&parent.as_str()) {
                        continue;
                    }
                    let filter = columns
                        .iter()
                        .enumerate()
                        .map(|(i, column)| format!("{column} = ?{}", i + 1))
                        .collect::<Vec<_>>()
                        .join(" AND ");
                    let plan: Vec<String> = conn
                        .prepare(&format!(
                            "EXPLAIN QUERY PLAN SELECT 1 FROM {child} WHERE {filter}"
                        ))?
                        .query_map(params_from_iter(vec![1; columns.len()]), |r| r.get(3))?
                        .collect::<Result<_>>()?;
                    if plan.iter().any(|step| step.starts_with("SCAN")) {
                        unindexed
                            .push(format!("{child} ({}) → {parent}: {plan:?}", columns.join(", ")));
                    }
                }
            }
            Ok(unindexed)
        })
        .unwrap();
    assert!(unindexed.is_empty(), "{unindexed:#?}");
}

/// A server stopped part way through a deletion, some tables emptied and one part way, finishes
/// it at the next sweep, and the sweep leaves everyone else's rows as they were.
#[test]
fn a_deletion_cut_short_is_finished_by_the_next_sweep() {
    let (_temp, db) = open();
    mark(&db, GONE);
    let mut at = 0;
    for _ in 0..12 {
        at = write_step(&db, GONE, at).1;
    }
    let left = left(&db, GONE);
    assert!(!left.contains_key("plays.user_id"));
    assert_eq!(left.get("account_changes.user_id"), Some(&(MANY - 200)));
    let others = rows(&db, GONE);

    testing::sweep(&db, NOW);

    assert_eq!(self::left(&db, GONE), BTreeMap::new());
    assert_eq!(rows(&db, GONE), others);
}

/// The task changes nothing of an account that is active or suspended, or of one that does not
/// exist.
#[test]
fn a_live_account_is_never_emptied() {
    let (_temp, db) = open();
    db.write_blocking(|tx| {
        tx.execute("UPDATE users SET status = 'suspended' WHERE id = ?1", [OTHER])
    })
    .unwrap();
    let all = rows(&db, 0);
    for user in [GONE, OTHER, 99] {
        run(&db, user);
        let (done, _) = write_step(&db, user, 0);
        assert!(done);
    }
    assert_eq!(rows(&db, 0), all);
}

/// Every table that names an account, by a `user_id` column or a foreign key to `users`, is
/// emptied by the task, left to the final delete's cascade, or goes with its parent rows. So a
/// new table of an account's data cannot be forgotten, nor one whose rows outlive the account
/// for want of a foreign key, as `plugin_cursors`' would. The audit log, `admin_actions`, keeps
/// both accounts' IDs and names on purpose, with no key, and names neither by `user_id`.
#[test]
fn every_table_that_names_an_account_is_emptied() {
    let (_temp, db) = testing::open();
    let task: Vec<&str> = TABLES.iter().map(|t| t.0).collect();
    assert!(task.contains(&"plugin_cursors"), "it has no foreign key to delete it");
    let (named, cascading) = db
        .read_blocking(|conn| {
            let named: BTreeSet<String> =
                account_columns(conn)?.into_iter().map(|(table, _)| table).collect();
            let cascading: BTreeSet<String> = conn
                .prepare(
                    "SELECT m.name FROM sqlite_schema m, pragma_foreign_key_list(m.name) f \
                     WHERE m.type = 'table' AND f.\"table\" = 'users' \
                     AND f.on_delete IN ('CASCADE', 'SET NULL')",
                )?
                .query_map([], |r| r.get(0))?
                .collect::<Result<_>>()?;
            Ok((named, cascading))
        })
        .unwrap();
    let forgotten: Vec<&String> = named
        .iter()
        .filter(|&table| {
            let table = table.as_str();
            !task.contains(&table) && !CASCADE.contains(&table) && !WITH_PARENTS.contains(&table)
        })
        .collect();
    assert!(forgotten.is_empty(), "nothing empties {forgotten:?}");
    for table in CASCADE {
        assert!(cascading.contains(table), "the final delete does not reach {table}");
    }
}

/// Rows of the account written after the task emptied their table, as by a request already past
/// the gate or the player's and queues' saves, go with the account's row. `liked_songs::ensure`
/// and a run's feed starts write none for it. Once the row is gone, such a write fails alone:
/// another account's in the same write is saved, and another account's own write goes through.
#[test]
fn rows_written_late_go_with_the_account_row_and_after_it_fail_alone() {
    let (_temp, db) = open();
    mark(&db, GONE);
    run_to_last(&db, GONE);
    assert_eq!(
        left(&db, GONE).keys().collect::<Vec<_>>(),
        [
            "invites.created_by",
            "invites.redeemed_by",
            "library_access.user_id",
            "liked_songs_deleted.user_id",
            "player_states.user_id",
            "plugin_connections.user_id",
            "users",
        ]
    );

    db.write_blocking(|tx| tx.execute(PLAY, params!["late", GONE, None::<i64>, NOW])).unwrap();
    db.write_blocking(|tx| tx.execute(PLAYER_STATE, params![GONE, "late.0"])).unwrap();
    db.write_blocking(|tx| queue(tx, GONE, "live", &["late"])).unwrap();
    db.write_blocking(|tx| {
        tx.execute(
            "INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, \
             created_at, modified_at) VALUES (2999, 1, ?1, 'P', 'p', x'70', 0, 0)",
            [GONE],
        )?;
        tx.execute(
            "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
             VALUES (2999, 0, 1, 1, 0)",
            [],
        )
    })
    .unwrap();
    db.write_blocking(|tx| {
        liked_songs::ensure(tx)?;
        tx.execute("DELETE FROM playlists WHERE user_id = ?1 AND kind = 'liked'", [OTHER])?;
        liked_songs::ensure(tx)?;
        feed::start_epoch(tx).map(drop)
    })
    .unwrap();
    let late = left(&db, GONE);
    for key in ["plays.user_id", "play_events.user_id", "queues.user_id", "queue_lists.user_id"] {
        assert_eq!(late.get(key), Some(&1), "{key}");
    }
    assert_eq!(late.get("playlists.user_id"), Some(&1), "ensure made it no Liked Songs");
    assert_eq!(late.get("account_feed_starts.user_id"), None);
    let others_liked = "SELECT count(*) FROM playlists WHERE user_id = 3 AND kind = 'liked'";
    assert_eq!(value(&db, others_liked), 1, "ensure ran");

    let (done, _) = write_step(&db, GONE, TABLES.len());
    assert!(done);
    assert_eq!(left(&db, GONE), BTreeMap::new());

    let saved = db
        .write_blocking(|tx| {
            let mut upsert = tx.prepare(PLAYER_STATE)?;
            let gone = upsert.execute(params![GONE, "after.0"]).is_err();
            let other = upsert.execute(params![OTHER, "after.0"]).is_ok();
            Ok((gone, other))
        })
        .unwrap();
    assert_eq!(saved, (true, true));
    assert!(
        db.write_blocking(|tx| tx.execute(PLAY, params!["after", GONE, None::<i64>, NOW])).is_err()
    );
    db.write_blocking(|tx| tx.execute(PLAY, params!["after", OTHER, None::<i64>, NOW])).unwrap();
    assert_eq!(left(&db, GONE), BTreeMap::new());
    let other_state =
        "SELECT count(*) FROM player_states WHERE user_id = 3 AND item_id = 'after.0'";
    assert_eq!(value(&db, other_state), 1);
}
