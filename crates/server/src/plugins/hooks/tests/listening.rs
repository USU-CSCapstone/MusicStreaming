//! The listening hooks, `played` and `playing`, which act for each user who connected the
//! plugin.

use jewelcase_plugins::Event;
use jewelcase_plugins::Permission::{ListeningActivity, Played, Playing};
use jewelcase_plugins::PlayEnd::{Finished, Skipped};
use serde_json::json;

use super::super::playing::FRESH_MS;
use super::super::plays::plays;
use super::{
    HANDLED, HOUR, connected, deliver, due_for, one, plan, play, setup, start, tracks_by_user,
};
use crate::db::DbError;
use crate::plugins::personal::disconnect;
use crate::plugins::testing::{caller, connect, install, manifest};

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
        plays.iter().map(|p| (p.track.id, p.started_at_ms, p.listen_time_ms, p.end)).collect();
    assert_eq!(summary, [(11, 200, 30_000, Finished), (12, 220, 30_000, Skipped)], "only theirs");
    assert_eq!(plays[0].track.artists, ["Aurora Lane"]);
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "each play is delivered once");
    start(&db, "d", 7, 11, 300).await;
    assert!(deliver(&db, 1000, HANDLED).await.is_empty(), "still playing, so not played yet");
}

#[tokio::test]
async fn two_users_plays_under_one_id_each_reach_only_their_own_user() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Played]).await;
    connect(&db, "plugin", 8, json!({}), 100).await;
    play(&db, "same", 7, 11, 200, "finished").await;
    play(&db, "same", 8, 12, 210, "skipped").await;
    let delivered = tracks_by_user(deliver(&db, 0, HANDLED).await);
    assert_eq!(delivered, [(7, vec![11]), (8, vec![12])], "each their own play, and only it");
}

#[tokio::test]
async fn a_connection_to_another_plugin_sends_this_one_nothing() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Played, Playing]).await;
    install(&db, manifest("other", &[])).await;
    // Connected since the start, and found first by a lookup that forgets the plugin.
    for user in [7, 8] {
        connect(&db, "other", user, json!({}), 0).await;
    }
    play(&db, "early", 7, 11, 50, "finished").await;
    play(&db, "theirs", 8, 12, 200, "finished").await;
    start(&db, "playing", 8, 11, 210).await;
    play(&db, "later", 7, 12, 220, "finished").await;
    let Event::Played(plays) = one(deliver(&db, 1000, HANDLED).await, 7) else {
        panic!("not plays")
    };
    let started: Vec<_> = plays.iter().map(|p| p.started_at_ms).collect();
    assert_eq!(started, [220], "only since they connected this one");

    // Found due just before they disconnected it, so only the other connection is left.
    start(&db, "now", 7, 11, 1000).await;
    db.write(|tx| disconnect(tx, "plugin", caller(7, false))).await.unwrap();
    for hook in [Played, Playing] {
        let due = due_for(hook, 7);
        let (_, nothing) = db.read(move |conn| (due.read)(conn, &due, 1000)).await.unwrap();
        assert!(nothing.is_none(), "{hook:?} after disconnecting: {nothing:?}");
    }
}

#[tokio::test]
async fn one_users_failures_never_disable_the_plugin() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Played]).await;
    play(&db, "a", 7, 11, 200, "finished").await;
    let mut now = 0;
    for attempt in 0..=73 {
        let tried = one(deliver(&db, now, Err("Invalid token")).await, 7);
        assert!(matches!(tried, Event::Played(_)), "attempt {attempt} still reaches it");
        now += HOUR;
    }
    assert!(deliver(&db, now - 1, HANDLED).await.is_empty(), "retried hourly");
    assert!(matches!(one(deliver(&db, now, HANDLED).await, 7), Event::Played(_)));
}

/// A stored end the server does not know cannot be read, rather than reaching the plugin as some
/// other end.
#[tokio::test]
async fn an_unknown_stored_end_is_an_error() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Played]).await;
    play(&db, "a", 7, 11, 200, "finished").await;
    db.write(|tx| {
        tx.execute_batch(
            "PRAGMA ignore_check_constraints = ON;
             UPDATE plays SET ended = 'paused';
             PRAGMA ignore_check_constraints = OFF;",
        )
    })
    .await
    .unwrap();
    let read = db.read(|conn| plays(conn, "plugin", 1, 7, 0)).await;
    assert!(
        matches!(read, Err(DbError::Sqlite(rusqlite::Error::FromSqlConversionFailure(15, ..)))),
        "{:?}",
        read.map(|(next, plays)| (next, plays.len()))
    );
}

#[tokio::test]
async fn plays_and_starts_need_listening_activity_as_well_as_the_hook() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[Played, Playing]).await;
    start(&db, "a", 7, 11, 200).await;
    play(&db, "b", 7, 12, 210, "finished").await;
    assert!(deliver(&db, 1000, HANDLED).await.is_empty());
}

/// The track and start time `event` announces as playing.
fn now_playing(event: Event) -> (u64, u64) {
    let Event::Playing(now) = event else { panic!("not playing: {event:?}") };
    (now.track.id, now.started_at_ms)
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
async fn two_users_starts_under_one_id_each_reach_only_their_own_user() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Playing]).await;
    connect(&db, "plugin", 8, json!({}), 100).await;
    start(&db, "same", 7, 11, 200).await;
    start(&db, "same", 8, 12, 210).await;
    let delivered = tracks_by_user(deliver(&db, 1000, HANDLED).await);
    assert_eq!(delivered, [(7, vec![11]), (8, vec![12])], "each their own track");

    start(&db, "next", 7, 11, 300).await;
    play(&db, "next", 8, 12, 310, "finished").await;
    let delivered = tracks_by_user(deliver(&db, 1000, HANDLED).await);
    assert_eq!(delivered, [(7, vec![11])], "the other user's play under the ID has ended");
}

#[tokio::test]
async fn a_deleted_users_plays_take_their_starts_and_ends_and_no_one_elses() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[]).await;
    // User 7's play was reported playing before it ended, so it has a start as well as an end.
    play(&db, "same", 7, 11, 200, "finished").await;
    start(&db, "same", 8, 12, 210).await;
    let left = db
        .write(|tx| {
            tx.execute("DELETE FROM users WHERE id = 7", [])?;
            let rows = |sql: &str| -> rusqlite::Result<Vec<(i64, String)>> {
                tx.prepare(sql)?.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
            };
            Ok((
                rows("SELECT user_id, id FROM plays")?,
                rows("SELECT user_id, play_id FROM play_events WHERE kind = 'started'")?,
                rows("SELECT user_id, play_id FROM play_events WHERE kind = 'ended'")?,
            ))
        })
        .await
        .unwrap();
    let theirs = vec![(8, "same".to_owned())];
    assert_eq!(left, (theirs.clone(), theirs, Vec::new()));
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
    db.write(|tx| {
        tx.execute_batch("UPDATE plays SET ended = 'skipped' WHERE id = 'c' AND user_id = 7")
    })
    .await
    .unwrap();
    assert!(deliver(&db, later, HANDLED).await.is_empty(), "skipped before it could be announced");
    // Found due just before it ended, it is read as nothing to deliver.
    let gone = due_for(Playing, 7);
    let (position, nothing) = db.read(move |conn| (gone.read)(conn, &gone, later)).await.unwrap();
    assert!(nothing.is_none() && position == 0, "nothing to deliver, so nothing moves");

    // A device catching up after being offline reports its plays already ended.
    play(&db, "offline", 7, 11, later, "finished").await;
    assert!(deliver(&db, later, HANDLED).await.is_empty(), "never playing as far as we knew");
}

#[tokio::test]
async fn starts_an_hour_old_are_cleared_away_and_ends_are_kept() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Playing]).await;
    play(&db, "ended", 7, 11, 0, "finished").await;
    start(&db, "old", 7, 11, 0).await;
    start(&db, "new", 7, 12, HOUR + 1).await;
    let kept: Vec<(String, String)> = db
        .read(|conn| {
            conn.prepare("SELECT play_id, kind FROM play_events ORDER BY seq")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
        .await
        .unwrap();
    let kept: Vec<_> = kept.iter().map(|(play, kind)| (play.as_str(), kind.as_str())).collect();
    assert_eq!(kept, [("ended", "ended"), ("new", "started")], "the end is still to be delivered");
}

/// The sweep mirrors the `plays_first_report` trigger's in `0005_activity.sql`, which runs on
/// every new play: it reaches the old starts through `play_events_started_age`, which holds
/// starts alone, rather than reading every event.
#[tokio::test]
async fn clearing_old_starts_reads_only_them() {
    let (_temp, db, _plugins) = setup().await;
    let sweep = "DELETE FROM play_events WHERE kind = 'started' AND reported_at < ?1 - 3600000";
    let steps: Vec<String> = db
        .read(move |conn| {
            conn.prepare(&format!("EXPLAIN QUERY PLAN {sweep}"))?
                .query_map([0], |row| row.get(3))?
                .collect()
        })
        .await
        .unwrap();
    // Covering or not is the planner's choice, which depends on the version and statistics.
    let [step] = steps.as_slice() else { panic!("{steps:#?}") };
    assert!(step.starts_with("SEARCH play_events USING"), "{step}");
    assert!(step.ends_with("INDEX play_events_started_age (reported_at<?)"), "{step}");
    let index: String = db
        .read(|conn| {
            conn.query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'play_events_started_age'",
                [],
                |row| row.get(0),
            )
        })
        .await
        .unwrap();
    assert!(index.ends_with("WHERE kind = 'started'"), "it holds no end: {index}");
}

/// Each listening hook finds itself due from its own kind of play event alone, so one that has
/// caught up never steps over the other's: the ends a device reports as it catches up after
/// being offline, or the starts of someone listening.
#[tokio::test]
async fn each_listening_hook_reads_only_its_own_kind_of_play_event() {
    let (_temp, db, _plugins) = setup().await;
    let feed = "USING INDEX play_events_feed (library_id=? AND user_id=? AND kind=? AND seq>?)";
    for (hook, event) in [(Played, "SEARCH e "), (Playing, "SEARCH st ")] {
        let steps = plan(&db, hook).await;
        let reads = |step: &String| step.starts_with(event) && step.ends_with(feed);
        assert!(steps.iter().any(reads), "{hook:?}: {steps:#?}");
    }
}
