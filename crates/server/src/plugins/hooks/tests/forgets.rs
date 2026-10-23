//! Requests to forget searches reach the plugin even where their user can no longer be acted
//! for, in a run with the shared settings alone, while a search held back for that is passed
//! over.

use jewelcase_plugins::Permission::{SearchActivity, Searched};
use jewelcase_plugins::{Event, Forgotten};

use rusqlite::OptionalExtension;

use super::{HANDLED, Received, connected, deliver, noted, setup};
use crate::api::testing::eventually;
use crate::db::testing::access;
use crate::db::{Database, now_ms};
use crate::plugins::personal::set_search_sharing;
use crate::plugins::testing::caller;
use crate::plugins::tests::TOUCHER;

/// Turns user 7's sharing of their searches with the plugin on or off.
async fn share(db: &Database, on: bool) {
    db.try_write(move |tx| set_search_sharing(tx, "plugin", caller(7, false), on)).await.unwrap();
}

/// Sets user 7's account to `status`.
async fn status(db: &Database, status: &'static str) {
    let sql = "UPDATE users SET status = ?1 WHERE id = 7";
    db.write(move |tx| tx.execute(sql, [status])).await.unwrap();
}

/// Each delivery in `received`, which must all be requests to forget user 7's searches, by
/// library: the library, the searches named (none for every search), and the user whose personal
/// settings its run had.
fn forgets(received: Vec<Received>) -> Vec<(i64, Option<Vec<u64>>, Option<i64>)> {
    let mut forgets: Vec<_> = received
        .into_iter()
        .map(|delivery| match delivery {
            Received {
                hook: Searched,
                library,
                user: 7,
                event: Some(Event::SearchesForgotten(Forgotten { user: 7, searches })),
                acting_for,
            } => (library, searches, acting_for),
            other => panic!("not a request to forget for user 7: {other:?}"),
        })
        .collect();
    forgets.sort_by_key(|(library, ..)| *library);
    forgets
}

/// Access to library 1 is revoked, then sharing stops: the plugin is still asked to forget there,
/// in a run that is not theirs, as well as in library 2, which they still reach, and is never
/// sent the search in library 1 it had not been sent yet, as it was no longer shared.
#[tokio::test]
async fn a_forget_reaches_the_plugin_after_the_library_is_revoked() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[SearchActivity, Searched]).await;
    plugins.set_enabled("plugin".into(), 2, true).await.unwrap();
    db.write(|tx| access(tx, 7, 2)).await.unwrap();
    share(&db, true).await;
    noted(&db, 7, 1, 1, true, now_ms()).await;
    let sent = deliver(&db, 0, HANDLED).await;
    assert_eq!(sent.iter().map(|r| r.acting_for).collect::<Vec<_>>(), [Some(7)], "{sent:?}");

    noted(&db, 7, 1, 2, true, now_ms()).await;
    let revoke = "DELETE FROM library_access WHERE user_id = 7 AND library_id = 1";
    db.write(move |tx| tx.execute(revoke, [])).await.unwrap();
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "the search waits while they lack access");
    share(&db, false).await;
    let everywhere = [(1, None, None), (2, None, None)];
    assert_eq!(forgets(deliver(&db, 0, HANDLED).await), everywhere);
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "the unsent search stays unsent");
}

/// Requests to forget that a suspended account made before it was suspended, to forget one
/// search or every one, still reach the plugin, in runs that are not theirs, and a search held
/// back before such a request is passed over.
#[tokio::test]
async fn a_forget_reaches_the_plugin_while_the_account_is_suspended() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[SearchActivity, Searched]).await;
    share(&db, true).await;
    noted(&db, 7, 1, 1, true, now_ms()).await;
    deliver(&db, 0, HANDLED).await;
    noted(&db, 7, 1, 2, true, now_ms()).await;
    noted(&db, 7, 1, 1, false, now_ms()).await;
    status(&db, "suspended").await;
    assert_eq!(forgets(deliver(&db, 0, HANDLED).await), [(1, Some(vec![1]), None)]);
    status(&db, "active").await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "the held search was passed over");

    share(&db, false).await;
    status(&db, "suspended").await;
    assert_eq!(forgets(deliver(&db, 0, HANDLED).await), [(1, None, None)]);
}

/// A request to forget runs without the user's personal settings, so one whose stored settings
/// cannot be read still reaches the plugin, here one that answers "done" to any event.
#[tokio::test]
async fn a_forget_runs_without_the_users_personal_settings() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[SearchActivity, Searched]).await;
    share(&db, true).await;
    share(&db, false).await;
    db.write(|tx| {
        tx.execute_batch(
            "PRAGMA ignore_check_constraints = ON; \
             UPDATE plugin_connections SET settings = 'not JSON' WHERE user_id = 7; \
             PRAGMA ignore_check_constraints = OFF;",
        )
    })
    .await
    .unwrap();
    let file = plugins.path("plugin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, TOUCHER).unwrap();

    plugins.dispatch().await;
    let sql = "SELECT l.last_run_summary FROM plugin_cursors c \
               JOIN plugin_libraries l USING (plugin_id, library_id)";
    let summary: String = eventually("delivered", async || {
        db.read(move |conn| conn.query_row(sql, [], |row| row.get(0)).optional()).await.unwrap()
    })
    .await;
    assert!(summary.starts_with("done ("), "{summary}");
}
