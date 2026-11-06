//! The `marked` hook, which tells a plugin the likes and hides of the users who connected it.

use jewelcase_plugins::MarkKind::{Hidden, Liked, Unliked};
use jewelcase_plugins::Permission::{LikesAndHides, ListeningActivity, Marked};
use jewelcase_plugins::{Event, Mark, MarkKind};

use super::{HANDLED, connected, deliver, one, plan, setup};
use crate::db::Database;

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

/// The marks of the one delivery for user 7, as (track, kind, at).
fn summary(event: Event) -> Vec<(u64, MarkKind, u64)> {
    let Event::Marked(marks) = event else { panic!("not marks: {event:?}") };
    marks.iter().map(|Mark { track, kind, at_ms }| (track.id, *kind, *at_ms)).collect()
}

#[tokio::test]
async fn marks_reach_a_plugin_only_for_users_who_connected_it_and_only_since() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    marked(&db, 7, 11, "liked", 50).await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "marked before they connected");

    marked(&db, 7, 11, "unliked", 200).await;
    marked(&db, 8, 12, "liked", 210).await;
    marked(&db, 7, 12, "hidden", 220).await;
    let delivered = summary(one(deliver(&db, 0, HANDLED).await, 7));
    assert_eq!(delivered, [(11, Unliked, 200), (12, Hidden, 220)], "only theirs, in order");
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "each mark is delivered once");
}

#[tokio::test]
async fn a_mark_of_a_track_since_purged_is_passed_over() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    marked(&db, 7, 999, "liked", 200).await;
    marked(&db, 7, 11, "liked", 210).await;
    assert_eq!(summary(one(deliver(&db, 0, HANDLED).await, 7)), [(11, Liked, 210)]);
    marked(&db, 7, 999, "unliked", 220).await;
    let passed = deliver(&db, 0, HANDLED).await;
    assert!(passed.iter().all(|d| d.event.is_none()), "nothing to send: {passed:?}");
    marked(&db, 7, 12, "liked", 230).await;
    assert_eq!(summary(one(deliver(&db, 0, HANDLED).await, 7)), [(12, Liked, 230)]);
}

#[tokio::test]
async fn marks_go_in_batches_of_a_hundred() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[LikesAndHides, Marked]).await;
    for at in 0..150 {
        let kind = if at % 2 == 0 { "liked" } else { "unliked" };
        marked(&db, 7, 11, kind, 200 + at).await;
    }
    assert_eq!(summary(one(deliver(&db, 0, HANDLED).await, 7)).len(), 100);
    let rest = summary(one(deliver(&db, 0, HANDLED).await, 7));
    assert_eq!((rest.len(), rest.last().copied()), (50, Some((11, Unliked, 349))));
}

/// The hook needs likes and hides granted, which listening activity does not stand in for.
#[tokio::test]
async fn marks_need_likes_and_hides_granted() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Marked]).await;
    marked(&db, 7, 11, "liked", 200).await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty());
}

/// Finding the hook due reads one range of the feed for each connected user.
#[tokio::test]
async fn the_marked_hook_reads_its_feed_by_user() {
    let (_temp, db, _plugins) = setup().await;
    let feed = "USING INDEX mark_events_feed (library_id=? AND user_id=? AND seq>?)";
    let steps = plan(&db, Marked).await;
    let reads = |step: &String| step.starts_with("SEARCH e ") && step.ends_with(feed);
    assert!(steps.iter().any(reads), "{steps:#?}");
}
