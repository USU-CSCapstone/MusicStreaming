//! The listening hooks, `played` and `playing`, which act for each user who connected the
//! plugin.

use jewelcase_plugins::Event;
use jewelcase_plugins::Permission::{ListeningActivity, Played, Playing};
use jewelcase_plugins::PlayEnd::{Finished, Skipped};

use super::super::due::event;
use super::super::playing::FRESH_MS;
use super::super::{Due, Hook, MAX_FAILURES};
use super::{HANDLED, HOUR, connected, deliver, one, play, setup};
use crate::db::Database;

#[tokio::test]
async fn plays_reach_a_plugin_only_for_users_who_connected_it_and_only_since() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Played]).await;
    play(&db, "before", 7, 11, 50, "finished").await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "played before they connected");

    play(&db, "a", 7, 11, 200, "finished").await;
    play(&db, "b", 8, 12, 210, "finished").await;
    play(&db, "c", 7, 12, 220, "skipped").await;
    let Event::Played(plays) = one(deliver(&db, 0, HANDLED).await, 7) else { panic!("not plays") };
    let summary: Vec<_> =
        plays.iter().map(|p| (p.track.id, p.started_at, p.listen_time_ms, p.end)).collect();
    assert_eq!(summary, [(11, 200, 30_000, Finished), (12, 220, 30_000, Skipped)], "only theirs");
    assert_eq!(plays[0].track.artists, ["Aurora Lane"]);
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "each play is delivered once");
}

#[tokio::test]
async fn one_users_failures_never_disable_the_plugin() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Played]).await;
    play(&db, "a", 7, 11, 200, "finished").await;
    let mut now = 0;
    for attempt in 1..=MAX_FAILURES + 2 {
        let tried = one(deliver(&db, now, Err("Invalid token")).await, 7);
        assert!(matches!(tried, Event::Played(_)), "attempt {attempt} still reaches it");
        now += HOUR;
    }
    assert!(deliver(&db, now - 1, HANDLED).await.is_empty(), "retried hourly");
    assert!(matches!(one(deliver(&db, now, HANDLED).await, 7), Event::Played(_)));
}

#[tokio::test]
async fn plays_and_starts_need_listening_activity_as_well_as_the_hook() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[Played, Playing]).await;
    start(&db, "a", 7, 11, 200).await;
    play(&db, "b", 7, 12, 210, "finished").await;
    assert!(deliver(&db, 1000, HANDLED).await.is_empty());
}

/// User `user` starts track `track` in library 1 as play `id`, first reported at `at`, and has
/// not finished it.
async fn start(db: &Database, id: &'static str, user: i64, track: i64, at: i64) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO plays (id, user_id, library_id, track_id, started_at, listen_time_ms, \
                                context, origin, updated_at) \
             VALUES (?1, ?2, 1, ?3, ?4, 0, '{}', 'context', ?4)",
            rusqlite::params![id, user, track, at],
        )
    })
    .await
    .unwrap();
}

/// The track and start time `event` announces as playing.
fn now_playing(event: Event) -> (u64, u64) {
    let Event::Playing(now) = event else { panic!("not playing: {event:?}") };
    (now.track.id, now.started_at)
}

#[tokio::test]
async fn only_the_newest_start_reaches_a_user_who_connected_it() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Playing]).await;
    start(&db, "before", 7, 11, 50).await;
    assert!(deliver(&db, 60, HANDLED).await.is_empty(), "started before they connected");

    start(&db, "a", 7, 11, 200).await;
    start(&db, "b", 7, 12, 210).await;
    start(&db, "c", 8, 12, 220).await;
    let newest = now_playing(one(deliver(&db, 1000, HANDLED).await, 7));
    assert_eq!(newest, (12, 210), "theirs, and the newest, passing over the first");
    assert!(deliver(&db, 1000, HANDLED).await.is_empty(), "each start is delivered once");
}

#[tokio::test]
async fn a_start_that_ended_or_went_stale_is_never_delivered() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Playing]).await;
    start(&db, "a", 7, 11, 200).await;
    assert!(deliver(&db, 200 + FRESH_MS + 1, HANDLED).await.is_empty(), "a minute is too late");
    start(&db, "b", 7, 12, 300).await;
    let fresh = now_playing(one(deliver(&db, 300 + FRESH_MS, HANDLED).await, 7));
    assert_eq!(fresh, (12, 300), "a minute is in time");

    let later = 300 + 2 * FRESH_MS;
    start(&db, "c", 7, 11, later).await;
    db.write(|tx| tx.execute_batch("UPDATE plays SET ended = 'skipped' WHERE id = 'c'"))
        .await
        .unwrap();
    assert!(deliver(&db, later, HANDLED).await.is_empty(), "skipped before it could be announced");
    // Found due just before it ended, it is read as nothing to deliver.
    let gone = Due {
        hook: Hook::Playing,
        plugin: "plugin".into(),
        library: 1,
        position: 0,
        failures: 0,
        user: 7,
    };
    let (position, nothing) = db.read(move |conn| event(conn, &gone, later)).await.unwrap();
    assert!(nothing.is_none() && position == 0, "nothing to deliver, so nothing moves");

    // A device catching up after being offline reports its plays already ended.
    play(&db, "offline", 7, 11, later, "finished").await;
    assert!(deliver(&db, later, HANDLED).await.is_empty(), "never playing as far as we knew");
}

#[tokio::test]
async fn starts_an_hour_old_are_cleared_away() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Playing]).await;
    start(&db, "old", 7, 11, 0).await;
    start(&db, "new", 7, 12, HOUR + 1).await;
    let kept: Vec<String> = db
        .read(|conn| {
            conn.prepare("SELECT play_id FROM play_starts")?
                .query_map([], |row| row.get(0))?
                .collect()
        })
        .await
        .unwrap();
    assert_eq!(kept, ["new"]);
}
