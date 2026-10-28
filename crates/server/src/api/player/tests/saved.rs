//! Where each account was, saved to resume from after the server restarts.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::super::Players;
use crate::api::AppState;
use crate::api::queue::Queues;
use crate::api::realtime::Hub;
use crate::api::scope::Caller;
use crate::api::testing::player::{item as item_id, progress_report, start_queue};
use crate::api::testing::{TOKEN, add_user, app_with_tracks, conn, router_over, state};
use crate::db::Database;

const OWNER: Caller = Caller { user: 1, device: 1, admin: true };

/// Reports to `players` from connection 7 of device 1 of the owner that item `item` is at
/// `status` and `position_ms`.
fn report_to(players: &Players, status: &str, item: usize, position_ms: u32) {
    let changes = json!({ "itemId": item_id(item), "positionMs": position_ms, "start": true });
    let report = serde_json::from_value(progress_report(status, changes));
    players.report(&Hub::default(), OWNER, 7, report.unwrap()).unwrap();
}

/// The owner's state saved in `db`, as a restarted server reads it, without `positionAt`.
async fn restarted(db: &Database) -> Value {
    let mut state = serde_json::to_value(Players::load(db).await.unwrap().current(1)).unwrap();
    state.as_object_mut().unwrap().remove("positionAt");
    state
}

#[tokio::test]
async fn playback_resumes_where_it_was_after_a_restart() {
    let (_temp, db, _app) = app_with_tracks().await;
    let players = Players::default();
    report_to(&players, "playing", 0, 0);
    report_to(&players, "paused", 1, 2500);
    players.save(&db).await;
    assert_eq!(
        restarted(&db).await,
        json!({
            "status": "stopped", "activeDeviceId": null, "itemId": item_id(1),
            "positionMs": 2500, "volume": null,
        }),
        "no device is connected to be playing yet"
    );

    // Saved again only once something changes.
    db.write(|tx| tx.execute("DELETE FROM player_states", [])).await.unwrap();
    players.save(&db).await;
    assert_eq!(restarted(&db).await["itemId"], json!(null));
}

#[tokio::test]
async fn playing_is_saved_as_far_as_it_has_played() {
    let (_temp, db, _app) = app_with_tracks().await;
    let players = Players::default();
    report_to(&players, "playing", 0, 1000);
    players.lock().get_mut(&1).unwrap().position_at -= 3000;
    // As far as it has played: the 3 seconds put behind it, and however long the test took.
    let saved = async || restarted(&db).await["positionMs"].as_u64().unwrap();
    players.save(&db).await;
    assert!((4000..5000).contains(&saved().await));

    players.lock().get_mut(&1).unwrap().position_at -= 2000;
    players.save(&db).await;
    assert!((6000..7000).contains(&saved().await), "and again as it plays on");
}

#[tokio::test]
async fn playing_on_through_an_item_is_saved_only_now_and_then() {
    let (_temp, db, _app) = app_with_tracks().await;
    let players = Players::default();
    let save = async || players.save_where(&db, super::super::saved::SAVE_PLAYING_EVERY).await;
    let saved = async || restarted(&db).await["positionMs"].as_u64().unwrap() / 1000;
    // As if `by` passed since it was last saved.
    let wait = |by| players.lock().get_mut(&1).unwrap().saved.as_mut().unwrap().2 -= by;
    report_to(&players, "playing", 0, 1000);
    save().await;
    assert_eq!(saved().await, 1, "at once as it starts");

    wait(Duration::from_secs(29));
    report_to(&players, "playing", 0, 30_000);
    save().await;
    assert_eq!(saved().await, 1, "not as it plays on, reported or not");
    wait(super::super::saved::SAVE_PLAYING_EVERY);
    save().await;
    assert_eq!(saved().await, 30, "until long enough after");

    report_to(&players, "playing", 0, 50_000);
    save().await;
    assert_eq!(saved().await, 50, "at once as it seeks");

    report_to(&players, "paused", 0, 40_000);
    save().await;
    assert_eq!(saved().await, 40, "at once as it pauses");
    report_to(&players, "playing", 1, 0);
    save().await;
    assert_eq!(restarted(&db).await["itemId"], json!(item_id(1)), "or moves to another item");
}

#[tokio::test]
async fn only_an_active_accounts_place_is_resumed() {
    let (temp, db, _app) = app_with_tracks().await;
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    conn.execute_batch(
        "INSERT INTO player_states VALUES (1, 'a.0', 0), (2, 'a.0', 0);
         UPDATE users SET status = 'suspended' WHERE id = 2;",
    )
    .unwrap();
    let players = Players::load(&db).await.unwrap();
    assert_eq!(players.current(1).item_id.as_deref(), Some("a.0"));
    assert_eq!(players.current(2).item_id, None, "suspended");
}

/// The place and the queue come back together after a restart: the state names an item the
/// queue has.
#[tokio::test]
async fn after_a_restart_the_state_names_an_item_the_restored_queue_has() {
    let (temp, db, _app) = app_with_tracks().await;
    let (players, queues) = (Arc::new(Players::default()), Arc::new(Queues::default()));
    let state = AppState { players: players.clone(), queues, ..state(&temp, db.clone()) };
    let (app, queues) = (router_over(state.clone()), state.queues);
    start_queue(&app, TOKEN).await;
    report_to(&players, "playing", 1, 2500);
    players.save(&db).await;
    queues.save(&db).await;

    let (players, queues) = (Players::load(&db).await.unwrap(), Queues::load(&db).await.unwrap());
    let state = players.current(1);
    assert_eq!((state.item_id.as_deref(), state.position_ms), (Some(item_id(1).as_str()), 2500));
    assert!(queues.holds(1, &item_id(1)));
}

#[tokio::test]
async fn one_accounts_place_failing_to_save_keeps_no_one_elses_from_saving() {
    let (temp, db, _app) = app_with_tracks().await;
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    let players = Players::default();
    report_to(&players, "paused", 1, 1000);
    let caller = Caller { user: 9, device: 2, admin: false };
    let report = serde_json::from_value(progress_report("paused", json!({ "start": true })));
    // An account deleted after the report was taken.
    players.report(&Hub::default(), caller, 8, report.unwrap()).unwrap();
    players.save(&db).await;
    assert_eq!(restarted(&db).await["positionMs"], json!(1000));
}
