//! The `marked` hook, which tells a plugin the likes and hides of the users who connected it:
//! first those they have, then each change.

use jewelcase_plugins::MarkKind::{Hidden, Liked, Unliked};
use jewelcase_plugins::Permission::{LikesAndHides, ListeningActivity, Marked};
use jewelcase_plugins::{Event, Mark, MarkKind};
use rusqlite::OptionalExtension;
use serde_json::json;

use super::super::due::all;
use super::super::{Due, Next, Ran, record};
use super::{HANDLED, Received, connected, deliver, plan, setup};
use crate::api::testing::eventually;
use crate::db::Database;
use crate::db::retention::{WINDOW_MS, sweep};
use crate::db::testing::Track;
use crate::plugins::personal::disconnect;
use crate::plugins::testing::{caller, connect};
use crate::plugins::tests::HEARER;

/// `user` marked track `track` of library 1 as `kind` at `at`, as the like and hide paths
/// record it.
async fn marked(db: &Database, user: i64, track: i64, kind: &'static str, at: i64) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO mark_events (user_id, library_id, track_id, kind, created_at) \
             VALUES (?1, 1, ?2, ?3, ?4)",
            rusqlite::params![user, track, kind, at],
        )
    })
    .await
    .unwrap();
}

/// `user` likes each of `tracks` of library 1, liked at `at` and on: they are in the Liked Songs
/// made for them, playlist 900 + `user`.
async fn likes(db: &Database, user: i64, tracks: Vec<i64>, at: i64) {
    db.write(move |tx| {
        tx.execute(
            "INSERT OR IGNORE INTO playlists (id, library_id, user_id, kind, title, folded, \
             sort_key, version, created_at, modified_at) \
             VALUES (900 + ?1, 1, ?1, 'liked', 'Liked Songs', 'liked songs', x'6c', 1, 0, 0)",
            [user],
        )?;
        for (key, track) in (0_i64..).zip(tracks) {
            tx.execute(
                "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
                 VALUES (900 + ?1, ?2, 1, ?3, ?4)",
                rusqlite::params![user, key, track, at + key],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

/// `user` hid track `track` of library 1 at `at`.
async fn hides(db: &Database, user: i64, track: i64, at: i64) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO hidden_tracks (user_id, library_id, track_id, hidden_at) \
             VALUES (?1, 1, ?2, ?3)",
            rusqlite::params![user, track, at],
        )
    })
    .await
    .unwrap();
}

/// The marks delivered for user 7, as (track, kind, at), and that nothing came for anyone else.
fn summary(received: Vec<Received>) -> Vec<(u64, MarkKind, u64)> {
    let mut marks = Vec::new();
    for delivery in received {
        match delivery.event {
            None => {}
            Some(Event::Marked(sent)) if delivery.user == 7 => {
                marks.extend(
                    sent.iter().map(|Mark { track, kind, at_ms }| (track.id, *kind, *at_ms)),
                );
            }
            Some(other) => panic!("not marks for user 7: {other:?}"),
        }
    }
    marks
}

#[tokio::test]
async fn a_user_is_first_sent_their_likes_and_hides_as_they_stand_then_each_change() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    likes(&db, 7, vec![11], 150).await;
    hides(&db, 7, 12, 160).await;
    marked(&db, 7, 11, "liked", 150).await;
    likes(&db, 8, vec![12], 170).await;
    let first = summary(deliver(&db, 0, HANDLED).await);
    assert_eq!(first, [(11, Liked, 150), (12, Hidden, 160)], "theirs, by track, not user 8's");
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [], "the feed so far is in them");

    marked(&db, 7, 11, "unliked", 300).await;
    marked(&db, 8, 12, "unliked", 310).await;
    marked(&db, 7, 12, "unhidden", 320).await;
    let changes = summary(deliver(&db, 0, HANDLED).await);
    assert_eq!(changes, [(11, Unliked, 300), (12, MarkKind::Unhidden, 320)]);
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), []);
}

#[tokio::test]
async fn a_change_while_the_likes_go_out_is_not_missed() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    db.write(|tx| {
        for id in 1001..=1150 {
            Track { path: format!("Many/{id}.flac"), ..Track::new(id, 1, 101) }.insert(tx)?;
        }
        Ok(())
    })
    .await
    .unwrap();
    likes(&db, 7, (1001..=1150).collect(), 200).await;
    let page = summary(deliver(&db, 0, HANDLED).await);
    assert_eq!((page.len(), page[0].0, page[99].0), (100, 1001, 1100), "a page of 100, by track");

    // Unliked once the page with it went out, and before the rest did.
    db.write(|tx| tx.execute("DELETE FROM playlist_items WHERE track_id = 1050", []))
        .await
        .unwrap();
    marked(&db, 7, 1050, "unliked", 500).await;
    let rest = summary(deliver(&db, 0, HANDLED).await);
    assert_eq!((rest.len(), rest[0].0, rest[49].0), (50, 1101, 1150));
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [(1050, Unliked, 500)], "from the feed");
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), []);
}

#[tokio::test]
async fn a_mark_of_a_track_since_purged_is_passed_over() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [], "nothing liked yet");
    marked(&db, 7, 999, "liked", 200).await;
    marked(&db, 7, 11, "liked", 210).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [(11, Liked, 210)]);
    marked(&db, 7, 999, "unliked", 220).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [], "nothing to send, and it moved on");
    marked(&db, 7, 12, "liked", 230).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [(12, Liked, 230)]);
}

#[tokio::test]
async fn marks_go_in_batches_of_a_hundred() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    deliver(&db, 0, HANDLED).await;
    for at in 0..150 {
        let kind = if at % 2 == 0 { "liked" } else { "unliked" };
        marked(&db, 7, 11, kind, 200 + at).await;
    }
    assert_eq!(summary(deliver(&db, 0, HANDLED).await).len(), 100);
    let rest = summary(deliver(&db, 0, HANDLED).await);
    assert_eq!((rest.len(), rest.last().copied()), (50, Some((11, Unliked, 349))));
}

/// Connecting again sends their likes and hides as they stand then, since what changed while
/// they were disconnected was never sent.
#[tokio::test]
async fn connecting_again_starts_from_where_the_user_stands() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    likes(&db, 7, vec![11], 150).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [(11, Liked, 150)]);
    db.write(|tx| disconnect(tx, "plugin", caller(7, false))).await.unwrap();
    hides(&db, 7, 12, 400).await;
    marked(&db, 7, 12, "hidden", 400).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [], "not connected");
    connect(&db, "plugin", 7, json!({}), 500).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [(11, Liked, 150), (12, Hidden, 400)]);
}

/// A plugin that had yet to hear of marks the sweep takes starts again from where its user
/// stands, so a plugin unable to take delivery for 30 days misses nothing.
#[tokio::test]
async fn marks_aged_out_unheard_start_the_user_again() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), []);
    likes(&db, 7, vec![11], 200).await;
    marked(&db, 7, 11, "liked", 200).await;
    sweep(&db, 200 + WINDOW_MS + 1).await;
    let gone: i64 = db
        .read(|conn| conn.query_row("SELECT count(*) FROM mark_events", [], |row| row.get(0)))
        .await
        .unwrap();
    assert_eq!(gone, 0, "the mark aged out");
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [(11, Liked, 200)], "sent as it stands");

    // One already heard of goes without starting anyone again.
    marked(&db, 7, 11, "unliked", 300).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [(11, Unliked, 300)]);
    sweep(&db, 300 + WINDOW_MS + 1).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), []);
}

/// User 7's `marked` delivery as it is found due and read, before the plugin runs.
async fn read(db: &Database) -> (Due, Next) {
    db.read(|conn| {
        let mut due = all(conn, 0)?;
        due.retain(|due| due.hook == Marked && due.user == Some(7));
        let due = due.pop().expect("a delivery is due for user 7");
        let (next, _) = (due.read)(conn, &due, 0)?;
        Ok((due, next))
    })
    .await
    .unwrap()
}

/// Records `due` as handled up to `next`, as a delivery does once the plugin has run.
async fn handled(db: &Database, due: Due, next: Next) {
    db.write(move |tx| record(tx, &due, next, Ran::Handled, "", 0)).await.unwrap();
}

/// A delivery still running as its user disconnects records nothing: connecting again starts
/// from where they stand then, and what they did while disconnected is never sent.
#[tokio::test]
async fn a_delivery_under_way_as_its_user_disconnects_leaves_them_disconnected() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    likes(&db, 7, vec![11], 150).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [(11, Liked, 150)]);
    hides(&db, 7, 12, 300).await;
    marked(&db, 7, 12, "hidden", 300).await;

    let (due, next) = read(&db).await;
    db.write(|tx| disconnect(tx, "plugin", caller(7, false))).await.unwrap();
    marked(&db, 7, 12, "unhidden", 400).await;
    db.write(|tx| tx.execute("DELETE FROM hidden_tracks", [])).await.unwrap();
    connect(&db, "plugin", 7, json!({}), 500).await;
    handled(&db, due, next).await;
    assert_eq!(
        summary(deliver(&db, 0, HANDLED).await),
        [(11, Liked, 150)],
        "as they stand, and not the unhide made while disconnected"
    );
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), []);
}

/// A delivery still running as the sweep takes marks it had yet to send records nothing, so the
/// user starts again from where they stand and the marks it took are not lost.
#[tokio::test]
async fn a_delivery_under_way_as_the_sweep_runs_starts_the_user_again() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), []);
    likes(&db, 7, vec![11], 200).await;
    marked(&db, 7, 11, "liked", 200).await;

    // The like is read; the hide comes as the plugin runs, and the sweep takes it unsent.
    let (due, next) = read(&db).await;
    hides(&db, 7, 12, 210).await;
    marked(&db, 7, 12, "hidden", 210).await;
    sweep(&db, 210 + WINDOW_MS + 1).await;
    handled(&db, due, next).await;
    assert_eq!(summary(deliver(&db, 0, HANDLED).await), [(11, Liked, 200), (12, Hidden, 210)]);
}

/// The hook needs likes and hides granted, which listening activity does not stand in for.
#[tokio::test]
async fn marks_need_likes_and_hides_granted() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Marked]).await;
    likes(&db, 7, vec![11], 150).await;
    marked(&db, 7, 11, "liked", 200).await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty());
}

/// Finding the hook due reads one range of the feed for each connected user.
#[tokio::test]
async fn the_marked_hook_reads_its_feed_by_user() {
    let (_temp, db, _plugins) = setup().await;
    let feed = "INDEX mark_events_feed (library_id=? AND user_id=? AND seq>?)";
    let steps = plan(&db, Marked).await;
    let reads = |step: &String| step.starts_with("SEARCH e ") && step.ends_with(feed);
    assert!(steps.iter().any(reads), "{steps:#?}");
}

/// A page of a user's likes and hides as they stand finds each side's next tracks by its index,
/// and joins only those to their tracks, so it costs what it holds, never what the user likes.
#[tokio::test]
async fn a_page_of_likes_and_hides_reads_from_their_indexes() {
    let (_temp, db, _plugins) = setup().await;
    let steps: Vec<String> = db
        .read(|conn| {
            conn.prepare(&format!("EXPLAIN QUERY PLAN {}", super::super::marks::standing_query()))?
                .query_map(rusqlite::params![1, 7, 0, 100], |row| row.get(3))?
                .collect()
        })
        .await
        .unwrap();
    let has = |step: &str| steps.iter().any(|s| s.contains(step));
    assert!(
        has("SEARCH i USING INDEX playlist_items_by_playlist_track (playlist_id=? AND track_id>?)"),
        "{steps:#?}"
    );
    assert!(
        has("SEARCH h USING PRIMARY KEY (user_id=? AND library_id=? AND track_id>?)"),
        "{steps:#?}"
    );
    // Only the pages each side found are scanned, and the page they make, `m`; no table.
    let scans_a_table =
        |s: &String| s.starts_with("SCAN ") && !s.starts_with("SCAN (") && s != "SCAN m";
    assert!(!steps.iter().any(scans_a_table), "{steps:#?}");
}

/// The whole path, with a plugin compiled from its component: a user's likes reach its `handle`
/// as a `marked` event, whose first mark names the track, and what it answers is recorded.
#[tokio::test]
async fn a_real_plugin_hears_a_users_likes() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    likes(&db, 7, vec![12, 11], 150).await;
    let file = plugins.path("plugin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, HEARER).unwrap();

    plugins.dispatch().await;
    let sql = "SELECT l.last_run_summary FROM plugin_cursors c \
               JOIN plugin_libraries l USING (plugin_id, library_id) WHERE c.hook = 'marked'";
    let summary: String = eventually("delivered", async || {
        db.read(move |conn| conn.query_row(sql, [], |row| row.get(0)).optional()).await.unwrap()
    })
    .await;
    // By track, so track 11 comes first.
    assert!(summary.starts_with("Track 11 ("), "{summary}");
}
