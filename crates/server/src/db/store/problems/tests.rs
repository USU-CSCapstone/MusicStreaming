//! Recording scan problems against a real database.

use std::sync::Arc;

use jewelcase_scanner::{Problem, ProblemKind};

use super::record;
use crate::db::Database;
use crate::db::store::SqliteStore;
use crate::db::testing::library;

/// A problem recorded again in the same group, with a detail that reads differently, keeps one
/// row, which takes the new detail and time.
#[test]
fn a_problem_seen_again_in_its_group_takes_the_new_detail_and_time() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&dir.path().join("jewelcase.db")).unwrap());
    let root = dir.path().join("music");
    let path = root.join("a/one.flac");
    db.write_blocking(move |tx| library(tx, 1, "Music", root)).unwrap();
    let store = SqliteStore::new(db).unwrap();
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
