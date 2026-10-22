//! A batch's problems cost what they change, not what their group holds
//! (`design/scanning.md` §13).

use std::path::Path;

use jewelcase_scanner::*;

use super::super::make;
use super::super::recompute::check;
use super::cost;

/// Problem `i`: the same corrupt audio as every other, so all of them share a group, in a file
/// of its own under `root`.
fn problem(root: &Path, i: usize) -> Problem {
    Problem {
        path: root.join(format!("Bad/{i:05}.flac")),
        kind: ProblemKind::CorruptAudio,
        detail: "cannot decode frame".to_owned(),
        seen_at: 1,
    }
}

/// Recording or clearing a problem moves its group's count by one rather than counting the group.
#[test]
fn a_problem_costs_what_it_changes_not_what_its_group_holds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    let recorded = |from: usize| Batch {
        problems: (from..from + 16).map(|i| problem(&root, i)).collect(),
        ..Batch::default()
    };
    let cleared = |from: usize| Batch {
        cleared: (from..from + 16).map(|i| problem(&root, i).path).collect(),
        ..Batch::default()
    };
    store.apply(1, recorded(0)).unwrap();

    // Recording 16 problems and clearing them, in a group of 16 and in one of 1,016.
    let small = [cost(&store, recorded(16)), cost(&store, cleared(16))];
    for start in (1000..2000).step_by(250) {
        let problems = (start..start + 250).map(|i| problem(&root, i)).collect();
        store.apply(1, Batch { problems, ..Batch::default() }).unwrap();
    }
    check(&store, "1,016 problems in a group");
    let large = [cost(&store, recorded(16)), cost(&store, cleared(16))];
    check(&store, "16 recorded and cleared again");
    for ((change, small), large) in ["recorded", "cleared"].into_iter().zip(small).zip(large) {
        // Counting the group's problems for each one changed would take at least 16,000 steps
        // more.
        assert!(large < small + 500, "{change}: {large} steps against {small}");
    }
}
