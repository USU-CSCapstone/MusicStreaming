//! Recording scan problems against a real database.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jewelcase_scanner::{Problem, ProblemKind};

use super::{clear_unopened, record};
use crate::db::Database;
use crate::db::store::SqliteStore;
use crate::db::testing::library;

/// A store in `dir` holding library 1, and that library's one root, `dir/music`.
fn store(dir: &Path) -> (SqliteStore, PathBuf) {
    let db = Arc::new(Database::open(&dir.join("jewelcase.db")).unwrap());
    let root = dir.join("music");
    let music = root.clone();
    db.write_blocking(move |tx| library(tx, 1, "Music", music)).unwrap();
    (SqliteStore::new(db).unwrap(), root)
}

/// A problem recorded again in the same group, with a detail that reads differently, keeps one
/// row, which takes the new detail and time.
#[test]
fn a_problem_seen_again_in_its_group_takes_the_new_detail_and_time() {
    let dir = tempfile::tempdir().unwrap();
    let (store, root) = store(dir.path());
    let path = root.join("a/one.flac");
    let problem = |detail: &str, seen_at| Problem {
        path: path.clone(),
        kind: ProblemKind::MalformedTags,
        detail: detail.to_owned(),
        seen_at,
    };

    record(&store, 1, problem("bad frame at offset 12", 100)).unwrap();
    record(&store, 1, problem("bad frame at offset 3456", 200)).unwrap();

    let (problems, group) = store
        .db
        .read_blocking(|conn| {
            let problems = conn.query_row(
                "SELECT count(*), max(detail), max(seen_at) FROM scan_problems",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)),
            )?;
            let group = conn.query_row(
                "SELECT count(*), max(count), max(last_seen_at) FROM scan_problem_groups",
                [],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
            )?;
            Ok::<_, rusqlite::Error>((problems, group))
        })
        .unwrap();
    assert_eq!(problems, (1, "bad frame at offset 3456".to_owned(), 200));
    assert_eq!(group, (1, 1, 200), "one group, holding one problem, seen last at 200");
}

/// A group whose count has gone wrong is deleted only once no problem is left in it, never with
/// the problems it still holds.
#[test]
fn a_group_goes_only_with_its_last_problem_whatever_its_count() {
    let dir = tempfile::tempdir().unwrap();
    let (store, root) = store(dir.path());
    let problem = |name: &str| Problem {
        path: root.join(name),
        kind: ProblemKind::Unreadable,
        detail: "cannot open".to_owned(),
        seen_at: 100,
    };
    record(&store, 1, problem("one.flac")).unwrap();
    record(&store, 1, problem("two.flac")).unwrap();
    store
        .db
        .write_blocking(|tx| tx.execute("UPDATE scan_problem_groups SET count = 1", []))
        .unwrap();

    clear_unopened(&store, 1, &root.join("one.flac")).unwrap();
    let left = |sql: &'static str| {
        store.db.read_blocking(move |conn| conn.query_row(sql, [], |r| r.get::<_, i64>(0))).unwrap()
    };
    assert_eq!(left("SELECT count(*) FROM scan_problems"), 1, "the other problem stays");
    assert_eq!(left("SELECT count(*) FROM scan_problem_groups"), 1, "and its group with it");

    clear_unopened(&store, 1, &root.join("two.flac")).unwrap();
    assert_eq!(left("SELECT count(*) FROM scan_problem_groups"), 0, "gone with its last problem");
}

/// A problem recorded again as another kind moves to that kind's group, both groups' counts
/// follow, and a group it leaves empty goes.
#[test]
fn a_problem_moves_between_groups_counted_out_of_one_and_into_the_other() {
    let dir = tempfile::tempdir().unwrap();
    let (store, root) = store(dir.path());
    let problem = |name: &str, kind| Problem {
        path: root.join(name),
        kind,
        detail: "cannot open".to_owned(),
        seen_at: 100,
    };
    let groups = || {
        store
            .db
            .read_blocking(|conn| {
                conn.prepare("SELECT kind, count FROM scan_problem_groups ORDER BY kind")?
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
    };
    let group = |kind: &str, count| (kind.to_owned(), count);
    record(&store, 1, problem("p.flac", ProblemKind::Unreadable)).unwrap();
    record(&store, 1, problem("q.flac", ProblemKind::Unreadable)).unwrap();
    assert_eq!(groups(), [group("unreadable", 2)]);

    record(&store, 1, problem("p.flac", ProblemKind::CorruptAudio)).unwrap();
    assert_eq!(groups(), [group("corruptAudio", 1), group("unreadable", 1)]);

    record(&store, 1, problem("q.flac", ProblemKind::CorruptAudio)).unwrap();
    assert_eq!(groups(), [group("corruptAudio", 2)]);
}
