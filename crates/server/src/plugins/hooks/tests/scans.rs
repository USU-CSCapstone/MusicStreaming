//! The hooks that run when a scan finishes, `scanFinished`, and as time passes, `schedule`.

use jewelcase_plugins::Event;
use jewelcase_plugins::Permission::{LibraryRead, ScanFinished, Schedule};

use super::{HANDLED, HOUR, deliver, one, setup};
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
