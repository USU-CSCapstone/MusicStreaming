//! The hooks that run when a scan finishes, `scanFinished`, and as time passes, `schedule`.

use jewelcase_plugins::Event;
use jewelcase_plugins::Permission::{LibraryRead, ScanFinished, Schedule};

use super::{HANDLED, HOUR, deliver, one, plan, setup};
use crate::plugins::testing::grant;

#[tokio::test]
async fn a_finished_scan_is_delivered_once_with_its_counts_added_up() {
    let (_temp, db, plugins) = setup().await;
    grant(&plugins, "plugin", &[], &[LibraryRead, ScanFinished]).await;
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO scans (id, library_id, trigger, state, scopes, added, updated, created_at, finished_at)
             VALUES (1, 1, 'watch', 'completed', '[]', 2, 1, 0, 100),
                    (2, 1, 'watch', 'completed', '[]', 3, 0, 0, 200),
                    (3, 1, 'watch', 'running', '[]', 9, 9, 0, NULL),
                    (4, 2, 'watch', 'completed', '[]', 9, 9, 0, 150);",
        )
    })
    .await
    .unwrap();
    assert!(deliver(&db, 100, HANDLED).await.is_empty(), "the first finished only now");

    let Event::ScanFinished(totals) = one(deliver(&db, 201, HANDLED).await, 0) else {
        panic!("not a finished scan")
    };
    assert_eq!(
        (totals.added, totals.updated),
        (5, 1),
        "both scans, not the running one, nor library 2's"
    );
    assert!(deliver(&db, 1000, HANDLED).await.is_empty(), "each is delivered once");

    // Finished in the millisecond the last delivery ran, so that delivery could not see it.
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO scans (id, library_id, trigger, state, scopes, added, updated, created_at, finished_at)
             VALUES (5, 1, 'watch', 'completed', '[]', 7, 0, 0, 201);",
        )
    })
    .await
    .unwrap();
    let Event::ScanFinished(late) = one(deliver(&db, 1000, HANDLED).await, 0) else {
        panic!("not a finished scan")
    };
    assert_eq!(late.added, 7, "only the scan it had not seen");
    assert!(deliver(&db, 1000, HANDLED).await.is_empty());
}

/// A finished scan is about the library, so it is delivered only to a plugin that may read it.
#[tokio::test]
async fn a_finished_scan_needs_library_read_as_well_as_the_hook() {
    let (_temp, db, plugins) = setup().await;
    grant(&plugins, "plugin", &[], &[ScanFinished]).await;
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO scans (id, library_id, trigger, state, scopes, added, updated, created_at, finished_at)
             VALUES (1, 1, 'watch', 'completed', '[]', 2, 1, 0, 100);",
        )
    })
    .await
    .unwrap();
    assert!(deliver(&db, 201, HANDLED).await.is_empty(), "not granted libraryRead");
    grant(&plugins, "plugin", &[], &[LibraryRead, ScanFinished]).await;
    let event = one(deliver(&db, 201, HANDLED).await, 0);
    assert!(matches!(event, Event::ScanFinished(_)), "{event:?}");
}

/// Scans are kept for months, mostly finished, so finding the hook due reads only the
/// library's completed scans after its position.
#[tokio::test]
async fn a_finished_scan_is_found_from_the_completed_scans_index() {
    let (_temp, db, _plugins) = setup().await;
    let steps = plan(&db, ScanFinished).await;
    let completed = "SEARCH s EXISTS USING COVERING INDEX scans_completed \
                     (library_id=? AND finished_at>? AND finished_at<?)";
    assert!(steps.iter().any(|step| step == completed), "{steps:#?}");
}

#[tokio::test]
async fn a_schedule_runs_when_its_interval_has_passed() {
    let (_temp, db, plugins) = setup().await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "not granted");
    grant(&plugins, "plugin", &[Schedule], &[]).await;
    let scheduled = |event| matches!(event, Event::Scheduled);
    assert!(scheduled(one(deliver(&db, 1000, HANDLED).await, 0)), "at once, the first time");
    assert!(deliver(&db, 1000 + HOUR - 1, HANDLED).await.is_empty());
    assert!(scheduled(one(deliver(&db, 1000 + HOUR, HANDLED).await, 0)), "an hour later");
}
