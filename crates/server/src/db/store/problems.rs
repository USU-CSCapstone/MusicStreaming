//! Scan problems: one row per path, grouped by kind, root, and normalized detail with a count
//! (`design/database.md` §8, `design/scanning.md` §13).

use std::path::Path;

use jewelcase_scanner::problems::group_key;
use jewelcase_scanner::{Problem, system_time_ms};
use rusqlite::{OptionalExtension, Result, Transaction, params};

use super::roots::Roots;

pub fn record_problem(
    roots: &Roots,
    tx: &Transaction<'_>,
    lib: i64,
    problem: &Problem,
) -> Result<()> {
    let Some((root_id, rel)) = roots.locate(lib, &problem.path) else {
        return Ok(());
    };
    let root_path = roots
        .0
        .read()
        .unwrap()
        .get(&lib)
        .and_then(|rs| rs.iter().find(|r| r.id == root_id))
        .map(|r| r.path.clone())
        .unwrap_or_default();
    let key = group_key(problem.kind, &root_path, &problem.detail);
    let seen = system_time_ms(problem.seen_at) as i64;
    let group_id: i64 = tx.query_row(
        "INSERT INTO scan_problem_groups (id, library_id, kind, group_key, summary, count, first_seen_at, last_seen_at) \
         VALUES (random() & 0x7FFFFFFFFFFFFFFF, ?1, ?2, ?3, ?4, 0, ?5, ?5) \
         ON CONFLICT (library_id, group_key) DO UPDATE SET last_seen_at = MAX(last_seen_at, excluded.last_seen_at) \
         RETURNING id",
        params![lib, problem.kind.as_str(), key, problem.detail, seen],
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
                    params![root_id, rel, problem.detail, seen],
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
                    params![lib, group_id, root_id, rel, problem.detail, seen],
                )?;
            recount_group(tx, group_id)?;
            if let Some(old) = existing {
                recount_group(tx, old)?;
            }
        }
    }
    Ok(())
}

pub fn clear_problem(roots: &Roots, tx: &Transaction<'_>, lib: i64, path: &Path) -> Result<()> {
    let Some((root_id, rel)) = roots.locate(lib, path) else {
        return Ok(());
    };
    clear_problem_rel(tx, root_id, &rel)
}

fn clear_problem_rel(tx: &Transaction<'_>, root_id: i64, rel: &str) -> Result<()> {
    let group: Option<i64> = tx
        .query_row(
            "SELECT group_id FROM scan_problems WHERE root_id = ?1 AND path = ?2",
            params![root_id, rel],
            |r| r.get(0),
        )
        .optional()?;
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
    tx.execute(
        "DELETE FROM scan_problem_groups WHERE id = ?1 AND count = 0",
        [group_id],
    )?;
    Ok(())
}
