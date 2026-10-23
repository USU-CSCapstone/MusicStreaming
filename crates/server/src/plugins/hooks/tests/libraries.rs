//! The hooks that act for users, whose connection and sharing hold in every library: each
//! library's activity still reaches only that library's deliveries.

use jewelcase_plugins::Permission::{
    LibraryRead, ListeningActivity, Played, Playing, SearchActivity, Searched,
};
use jewelcase_plugins::{Event, Permission};

use serde_json::json;

use super::{HANDLED, Received, connected, deliver, setup};
use crate::db::testing::{User, access};
use crate::db::{Database, now_ms};
use crate::plugins::personal::set_search_sharing;
use crate::plugins::testing::{caller, connect};

/// `user` ends a play of `track` in `library` at `at`.
async fn played(db: &Database, user: i64, library: i64, track: i64, at: i64) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO plays (id, user_id, library_id, track_id, title, artists, artist_keys, \
                                album_title_key, album_artists_key, genre_keys, duration_us, \
                                started_at, listen_time_ms, context, origin, updated_at, ended) \
             VALUES ('play ' || ?2, ?4, ?1, ?2, 'A', '[]', '[]', '', '', '[]', 0, ?3, 30000, \
                     '{}', 'context', ?3, 'finished')",
            rusqlite::params![library, track, at, user],
        )
    })
    .await
    .unwrap();
}

/// `user` settles on search `search` in `library` at `at`, or, unless `kept`, has removed it.
async fn noted(db: &Database, user: i64, library: i64, search: i64, kept: bool, at: i64) {
    db.write(move |tx| {
        if kept {
            tx.execute(
                "INSERT INTO recent_searches (id, user_id, library_id, query, folded, searched_at) \
                 VALUES (?2, ?4, ?1, 'words', 'words', ?3)",
                rusqlite::params![library, search, at, user],
            )?;
        }
        tx.execute(
            "INSERT INTO search_events (user_id, library_id, kind, search_id, found_tracks, \
                                        found_albums, found_artists, created_at) \
             VALUES (?5, ?1, iif(?3, 'searched', 'forgotten'), ?2, iif(?3, 1, NULL), \
                     iif(?3, 0, NULL), iif(?3, 0, NULL), ?4)",
            rusqlite::params![library, search, kept, at, user],
        )
    })
    .await
    .unwrap();
}

/// Each delivery's hook and library, and the IDs it named: the tracks played or playing, the
/// searches found or forgotten, or zero for a request to forget everything.
fn named(received: Vec<Received>) -> Vec<(Permission, i64, Vec<u64>)> {
    let ids = |event: Option<Event>| match event {
        Some(Event::Played(plays)) => plays.iter().map(|p| p.track.id).collect(),
        Some(Event::Playing(playing)) => vec![playing.track.id],
        Some(Event::Searched(found)) => found.iter().map(|s| s.id).collect(),
        Some(Event::SearchesForgotten(Some(ids))) => ids,
        Some(Event::SearchesForgotten(None)) => vec![0],
        other => panic!("{other:?}"),
    };
    let mut named: Vec<_> =
        received.into_iter().map(|r| (r.hook, r.library, ids(r.event))).collect();
    named.sort_by_key(|(hook, library, _)| (hook.name(), *library));
    named
}

#[tokio::test]
async fn a_users_activity_reaches_only_the_library_it_happened_in() {
    let (_temp, db, plugins) = setup().await;
    let plugin_wide = [ListeningActivity, Played, SearchActivity, Searched];
    connected(&plugins, &db, &plugin_wide).await;
    let per_library = vec![(1, vec![LibraryRead]), (2, vec![LibraryRead])];
    plugins.set_grants("plugin".into(), plugin_wide.to_vec(), per_library).await.unwrap();
    plugins.set_enabled("plugin".into(), 2, true).await.unwrap();
    db.write(|tx| access(tx, 7, 2)).await.unwrap();
    db.try_write(|tx| set_search_sharing(tx, "plugin", caller(7, false), true)).await.unwrap();
    let since = now_ms();
    // The forget comes first, so a delivery reading every library would begin with it.
    noted(&db, 7, 2, 2, false, since).await;
    noted(&db, 7, 1, 1, true, since).await;
    played(&db, 7, 1, 11, since).await;
    played(&db, 7, 2, 21, since).await;
    let expected = [
        (Played, 1, vec![11]),
        (Played, 2, vec![21]),
        (Searched, 1, vec![1]),
        (Searched, 2, vec![2]),
    ];
    assert_eq!(named(deliver(&db, 0, HANDLED).await), expected);

    noted(&db, 7, 2, 3, true, since).await;
    let expected = [(Searched, 2, vec![3])];
    assert_eq!(named(deliver(&db, 0, HANDLED).await), expected, "library 1 has nothing new");

    db.try_write(|tx| set_search_sharing(tx, "plugin", caller(7, false), false)).await.unwrap();
    let forget = [(Searched, 1, vec![0]), (Searched, 2, vec![0])];
    assert_eq!(named(deliver(&db, 0, HANDLED).await), forget, "in each library, once");
}

/// Nothing is delivered for a user in a library they have lost access to: neither what they
/// played there before, nor what they are playing, nor a request to forget written while they
/// reached it.
#[tokio::test]
async fn nothing_is_delivered_for_a_user_where_they_no_longer_reach() {
    let (_temp, db, plugins) = setup().await;
    let plugin_wide = [ListeningActivity, Played, Playing, SearchActivity, Searched];
    connected(&plugins, &db, &plugin_wide).await;
    let per_library = vec![(1, vec![LibraryRead]), (2, vec![LibraryRead])];
    plugins.set_grants("plugin".into(), plugin_wide.to_vec(), per_library).await.unwrap();
    plugins.set_enabled("plugin".into(), 2, true).await.unwrap();
    db.write(|tx| access(tx, 7, 2)).await.unwrap();
    for sharing in [true, false] {
        db.try_write(move |tx| set_search_sharing(tx, "plugin", caller(7, false), sharing))
            .await
            .unwrap();
    }
    let since = now_ms();
    played(&db, 7, 2, 21, since).await;
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO plays (id, user_id, library_id, track_id, title, artists, artist_keys, \
                                album_title_key, album_artists_key, genre_keys, duration_us, \
                                started_at, listen_time_ms, context, origin, updated_at) \
             VALUES ('now', 7, 2, 21, 'A', '[]', '[]', '', '', '[]', 0, ?1, 0, '{}', 'context', \
                     ?1)",
            [since],
        )?;
        tx.execute("DELETE FROM library_access WHERE user_id = 7 AND library_id = 2", [])
    })
    .await
    .unwrap();
    assert_eq!(named(deliver(&db, since, HANDLED).await), [(Searched, 1, vec![0])]);
}

/// An admin reaches every library without being granted any, so their activity there is
/// delivered.
#[tokio::test]
async fn an_admins_activity_is_delivered_without_a_grant_of_the_library() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Played, SearchActivity, Searched]).await;
    db.write(|tx| User::new(9, "admin").insert(tx)).await.unwrap();
    connect(&db, "plugin", 9, json!({}), 100).await;
    db.try_write(|tx| set_search_sharing(tx, "plugin", caller(9, true), true)).await.unwrap();
    let since = now_ms();
    played(&db, 9, 1, 11, since).await;
    noted(&db, 9, 1, 1, true, since).await;
    let delivered = deliver(&db, 0, HANDLED).await.into_iter().filter(|r| r.user == 9).collect();
    assert_eq!(named(delivered), [(Played, 1, vec![11]), (Searched, 1, vec![1])]);
}
