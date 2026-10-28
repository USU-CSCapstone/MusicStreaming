//! Queues saved to resume from after the server restarts.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::StatusCode;
use jewelcase_core::{Anchor, Edit, Source};
use serde_json::{Value, json};

use super::super::{Account, Queues};
use super::{KEY, OTHER, QUEUE, start};
use crate::api::testing::{TOKEN, app_with_tracks, call, eventually, state};
use crate::api::{AppState, Uuid, router};
use crate::db::Database;

/// A source of library 1 keyed `key`.
fn source(key: &str, tracks: &[&str]) -> Source {
    Source {
        key: key.into(),
        library_id: "1".into(),
        tracks: tracks.iter().map(|track| (*track).into()).collect(),
    }
}

/// Starts the owner's queue in `queues` as `queue`, playing `tracks` from the first.
fn begin(queues: &Queues, queue: &str, tracks: &[&str]) {
    let id = Uuid::try_from(queue.to_owned()).unwrap();
    queues
        .replace(1, id, json!({ "type": "album", "id": "101" }), source(KEY, tracks), 0, None)
        .unwrap();
}

/// Applies `edit` to the owner's queue in `queues`, and marks it unsaved, as an edit does.
fn edit(queues: &Queues, edit: &Edit) {
    let mut accounts = queues.lock();
    let account = accounts.get_mut(&1).unwrap();
    account.queue.apply(edit).unwrap();
    account.version += 1;
    account.unsaved = true;
}

/// The owner's queue as a restarted server reads it.
async fn restarted(db: &Database) -> Option<Account> {
    Queues::load(db).await.unwrap().accounts.into_inner().unwrap().remove(&1)
}

#[tokio::test]
async fn a_queue_resumes_where_it_was_after_a_restart() {
    let (_temp, db, _app) = app_with_tracks().await;
    let queues = Arc::new(Queues::default());
    begin(&queues, QUEUE, &["1", "2", "1"]);
    edit(&queues, &Edit::Add { source: source("m", &["2"]), before: Anchor::ManualEnd });
    edit(&queues, &Edit::Shuffle { on: true, seed: 3 });
    edit(&queues, &Edit::Next);
    queues.save(&db).await;

    let account = restarted(&db).await.unwrap();
    assert_eq!(account.queue_id.as_str(), QUEUE);
    assert_eq!(account.version, 3);
    assert_eq!(account.context, json!({ "type": "album", "id": "101" }));
    assert_eq!(account.queue, queues.lock()[&1].queue);
    assert!(!account.unsaved);
}

#[tokio::test]
async fn only_what_changed_is_saved() {
    let (_temp, db, _app) = app_with_tracks().await;
    let queues = Arc::new(Queues::default());
    begin(&queues, QUEUE, &["1", "2"]);
    queues.save(&db).await;
    // A source is written once: this one would be found again if it were rewritten.
    db.write(|tx| tx.execute("UPDATE queue_sources SET tracks = '[\"1\",\"2\",\"1\"]'", []))
        .await
        .unwrap();
    edit(&queues, &Edit::Next);
    queues.save(&db).await;
    let tracks = db
        .read(|conn| {
            conn.query_row("SELECT tracks FROM queue_sources", [], |row| row.get::<_, String>(0))
        })
        .await
        .unwrap();
    assert_eq!(tracks, r#"["1","2","1"]"#);
    assert_eq!(restarted(&db).await.unwrap().version, 1, "the edit is saved");

    // Nothing changed, nothing written.
    db.write(|tx| tx.execute("DELETE FROM queues", [])).await.unwrap();
    queues.save(&db).await;
    assert!(restarted(&db).await.is_none());
}

#[tokio::test]
async fn a_new_queue_replaces_the_saved_one_and_its_sources() {
    let (_temp, db, _app) = app_with_tracks().await;
    let queues = Arc::new(Queues::default());
    begin(&queues, QUEUE, &["1", "2"]);
    edit(&queues, &Edit::Add { source: source("m", &["2"]), before: Anchor::ManualEnd });
    queues.save(&db).await;
    begin(&queues, OTHER, &["2"]);
    queues.save(&db).await;

    let account = restarted(&db).await.unwrap();
    assert_eq!(account.queue_id.as_str(), OTHER);
    assert_eq!(account.version, 0);
    assert_eq!(account.queue.sources().len(), 2, "its own and the Manual Queue carried over");
    let held: i64 = db
        .read(|conn| conn.query_row("SELECT count(*) FROM queue_sources", [], |row| row.get(0)))
        .await
        .unwrap();
    assert_eq!(held, 2);
}

#[tokio::test]
async fn a_queue_goes_with_its_account() {
    let (_temp, db, _app) = app_with_tracks().await;
    let queues = Arc::new(Queues::default());
    begin(&queues, QUEUE, &["1"]);
    queues.save(&db).await;
    db.write(|tx| tx.execute("DELETE FROM users WHERE id = 1", [])).await.unwrap();
    let rows: i64 = db
        .read(|conn| {
            conn.query_row(
                "SELECT (SELECT count(*) FROM queues) + (SELECT count(*) FROM queue_sources)",
                [],
                |row| row.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn a_queue_that_will_not_read_back_is_left_out() {
    let (_temp, db, _app) = app_with_tracks().await;
    let queues = Arc::new(Queues::default());
    begin(&queues, QUEUE, &["1"]);
    queues.save(&db).await;
    db.write(|tx| tx.execute("UPDATE queues SET state = '{\"order\":5}'", [])).await.unwrap();
    assert!(restarted(&db).await.is_none());
}

/// The API over `db` in `temp`, with `queues` as the server's.
fn app_over(temp: &tempfile::TempDir, db: &Arc<Database>, queues: Arc<Queues>) -> Router {
    let state = AppState { queues, ..state(temp, db.clone()) };
    let peer = SocketAddr::from(([203, 0, 113, 1], 50_000));
    router(state).layer(MockConnectInfo(peer))
}

/// The owner's `getQueue` answer.
async fn snapshot(app: &Router) -> (StatusCode, Value) {
    call(app, TOKEN, "GET", "/api/v1/me/queue", None).await
}

/// A started queue is saved in the background, and as the server stops, and a server that
/// starts again serves it.
#[tokio::test]
async fn a_started_queue_is_served_after_a_restart() {
    let (temp, db, _app) = app_with_tracks().await;
    let queues = Arc::new(Queues::default());
    let app = app_over(&temp, &db, queues.clone());
    let saving = queues.keep_saved(db.clone());
    let body = Some(start(QUEUE, "1", &["1", "2"]));
    assert_eq!(call(&app, TOKEN, "POST", "/api/v1/me/queue", body).await.0, StatusCode::NO_CONTENT);
    let started = snapshot(&app).await.1;
    // Within a second or so, with nothing asking for it.
    eventually("the queue to be saved", async || restarted(&db).await).await;
    saving.abort();

    // And what the stop saves: a start made after the last periodic save.
    let body = Some(start(OTHER, "1", &["2"]));
    call(&app, TOKEN, "POST", "/api/v1/me/queue", body).await;
    queues.save(&db).await;
    let loaded = Arc::new(Queues::load(&db).await.unwrap());
    let (status, again) = snapshot(&app_over(&temp, &db, loaded)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again, snapshot(&app).await.1);
    assert_eq!(again["queueId"], OTHER);
    assert_ne!(again, started);
}

/// An earlier queue ID started again before the next save is a new queue, whatever the database
/// holds under that ID.
#[tokio::test]
async fn restarting_an_earlier_queue_replaces_what_was_saved() {
    let (_temp, db, _app) = app_with_tracks().await;
    let queues = Arc::new(Queues::default());
    begin(&queues, QUEUE, &["1", "2"]);
    edit(&queues, &Edit::Add { source: source("m", &["2"]), before: Anchor::ManualEnd });
    edit(&queues, &Edit::Add { source: source("n", &["1"]), before: Anchor::ContextEnd });
    queues.save(&db).await;
    begin(&queues, OTHER, &["2"]);
    begin(&queues, QUEUE, &["1"]);
    queues.save(&db).await;
    assert_eq!(restarted(&db).await.unwrap().queue, queues.lock()[&1].queue);

    // With as many sources as the saved one, so only their tracks say they differ.
    begin(&queues, OTHER, &["1", "2"]);
    queues.save(&db).await;
    begin(&queues, QUEUE, &["2"]);
    begin(&queues, OTHER, &["2", "2", "2"]);
    queues.save(&db).await;
    assert_eq!(restarted(&db).await.unwrap().queue, queues.lock()[&1].queue);
}
