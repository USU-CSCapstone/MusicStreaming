//! Scan problems: one row per path, grouped by kind, root, and normalized detail with a count
//! (`design/database.md` §8, `design/scanning.md` §13).

use std::path::{Path, PathBuf};

use jewelcase_scanner::{Problem, Scope, UNOPENED, problem_group_key};
use rusqlite::{Connection, OptionalExtension, Result, ToSql, Transaction, params};

use super::SqliteStore;
use super::names::name;
use super::roots::{Roots, bounds_under, join, relative};
use crate::db::DbError;

/// The absolute paths with a recorded problem that `scope` reaches ([`Scope::contains_file`]).
pub fn under(store: &SqliteStore, lib: i64, scope: &Scope) -> Result<Vec<PathBuf>, DbError> {
    let Some(root_id) = store.roots.root_by_path(lib, &scope.root) else {
        return Ok(Vec::new());
    };
    let rel = relative(&scope.root, &scope.path);
    let scope = scope.clone();
    store.db.read_blocking(move |conn| {
        let select = "SELECT path FROM scan_problems WHERE root_id = ?1";
        // A Directory scope reaches less deep than the range, so the scope filters the rows.
        let paths = match bounds_under(&rel) {
            None => texts(conn, select, params![root_id])?,
            Some([lower, upper]) => texts(
                conn,
                &format!("{select} AND path >= ?2 AND path < ?3"),
                params![root_id, lower, upper],
            )?,
        };
        Ok(paths
            .iter()
            .map(|path| join(&scope.root, path))
            .filter(|p| scope.contains_file(p))
            .collect())
    })
}

/// Records `problem` for `lib` in a transaction of its own, as a background job does.
pub fn record(store: &SqliteStore, lib: i64, problem: Problem) -> Result<(), DbError> {
    let roots = store.roots.clone();
    store.db.write_blocking(move |tx| record_problem(&roots, tx, lib, &problem))
}

/// Runs `sql` with `args` bound and returns its one text column.
fn texts(conn: &Connection, sql: &str, args: &[&dyn ToSql]) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(sql)?;
    stmt.query_map(args, |r| r.get(0))?.collect()
}

pub fn record_problem(
    roots: &Roots,
    tx: &Transaction<'_>,
    lib: i64,
    problem: &Problem,
) -> Result<()> {
    // Nothing to record under a root that has just been removed.
    let Some((root_id, rel)) = roots.locate(lib, &problem.path) else {
        return Ok(());
    };
    let Some(root_path) = roots.absolute(lib, root_id, "") else {
        return Ok(());
    };
    let key = problem_group_key(problem.kind, &root_path, &problem.detail);
    let group_id: i64 = tx.query_row(
        "INSERT INTO scan_problem_groups (id, library_id, kind, group_key, summary, count, first_seen_at, last_seen_at) \
         VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, 0, ?5, ?5) \
         ON CONFLICT (library_id, group_key) DO UPDATE SET last_seen_at = MAX(last_seen_at, excluded.last_seen_at) \
         RETURNING id",
        params![lib, name(problem.kind), key, problem.detail, problem.seen_at],
        |r| r.get(0),
    )?;
    let existing: Option<i64> = tx
        .query_row(
            "SELECT group_id FROM scan_problems WHERE root_id = ?1 AND path = ?2",
            params![root_id, rel],
            |r| r.get(0),
        )
        .optional()?;
    match existing {
        Some(old) if old == group_id => {
            tx.execute(
                    "UPDATE scan_problems SET detail = ?3, seen_at = ?4 WHERE root_id = ?1 AND path = ?2",
                    params![root_id, rel, problem.detail, problem.seen_at],
                )?;
        }
        _ => {
            // Insert into the new group before recounting the old one, so
            // a group is never emptied and deleted while still referenced.
            tx.execute(
                "DELETE FROM scan_problems WHERE root_id = ?1 AND path = ?2",
                params![root_id, rel],
            )?;
            tx.execute(
                    "INSERT INTO scan_problems (library_id, group_id, root_id, path, detail, seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![lib, group_id, root_id, rel, problem.detail, problem.seen_at],
                )?;
            recount_group(tx, group_id)?;
            if let Some(old) = existing {
                recount_group(tx, old)?;
            }
        }
    }
    Ok(())
}

/// Clears any problem recorded at `path`, if it lies under one of `lib`'s roots.
pub fn clear_problem(roots: &Roots, tx: &Transaction<'_>, lib: i64, path: &Path) -> Result<()> {
    let Some((root_id, rel)) = roots.locate(lib, path) else {
        return Ok(());
    };
    let group = tx
        .query_row(
            "SELECT group_id FROM scan_problems WHERE root_id = ?1 AND path = ?2",
            params![root_id, rel],
            |r| r.get(0),
        )
        .optional()?;
    clear_problem_rel(tx, root_id, &rel, group)
}

/// Clears the problem at `path` if it says the file could not be opened ([`UNOPENED`]), now
/// that a background job has opened it, in a transaction of its own.
pub fn clear_unopened(store: &SqliteStore, lib: i64, path: &Path) -> Result<(), DbError> {
    let Some((root_id, rel)) = store.roots.locate(lib, path) else {
        return Ok(());
    };
    store.db.write_blocking(move |tx| {
        let [unreadable, denied] = UNOPENED.map(name);
        let group = tx
            .query_row(
                "SELECT p.group_id FROM scan_problems p JOIN scan_problem_groups g ON g.id = p.group_id \
                 WHERE p.root_id = ?1 AND p.path = ?2 AND g.kind IN (?3, ?4)",
                params![root_id, rel, unreadable, denied],
                |r| r.get(0),
            )
            .optional()?;
        clear_problem_rel(tx, root_id, &rel, group)
    })
}

/// Deletes the problem at `rel` under `root_id` when `group`, the group it is in, is `Some`, and
/// recounts that group.
fn clear_problem_rel(
    tx: &Transaction<'_>,
    root_id: i64,
    rel: &str,
    group: Option<i64>,
) -> Result<()> {
    if let Some(group_id) = group {
        tx.execute(
            "DELETE FROM scan_problems WHERE root_id = ?1 AND path = ?2",
            params![root_id, rel],
        )?;
        recount_group(tx, group_id)?;
    }
    Ok(())
}

fn recount_group(tx: &Transaction<'_>, group_id: i64) -> Result<()> {
    tx.execute(
        "UPDATE scan_problem_groups SET count = (SELECT COUNT(*) FROM scan_problems WHERE group_id = ?1) WHERE id = ?1",
        [group_id],
    )?;
    tx.execute("DELETE FROM scan_problem_groups WHERE id = ?1 AND count = 0", [group_id])?;
    Ok(())
}
