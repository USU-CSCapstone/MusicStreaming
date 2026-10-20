//! The hooks that act for users, whose connection and sharing hold in every library: each
//! library's activity still reaches only that library's deliveries.

use jewelcase_plugins::Event;
use jewelcase_plugins::Permission::{
    LibraryRead, ListeningActivity, Played, SearchActivity, Searched,
};

use super::{HANDLED, Hook, Received, connected, deliver, setup};
use crate::db::testing::access;
use crate::db::{Database, now_ms};
use crate::plugins::personal::set_search_sharing;

/// User 7 ends a play of `track` in `library` at `at`.
async fn played(db: &Database, library: i64, track: i64, at: i64) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO plays (id, user_id, library_id, track_id, title, artists, started_at, \
                                listen_time_ms, context, origin, updated_at, ended) \
             VALUES ('play ' || ?2, 7, ?1, ?2, 'A', '[]', ?3, 30000, '{}', 'context', ?3, \
                     'finished')",
            rusqlite::params![library, track, at],
        )
    })
    .await
    .unwrap();
}

/// User 7 settles on search `search` in `library` at `at`, or, unless `kept`, has removed it.
async fn noted(db: &Database, library: i64, search: i64, kept: bool, at: i64) {
    db.write(move |tx| {
        if kept {
            tx.execute(
                "INSERT INTO recent_searches (id, user_id, library_id, query, folded, searched_at) \
                 VALUES (?2, 7, ?1, 'words', 'words', ?3)",
                rusqlite::params![library, search, at],
            )?;
        }
        tx.execute(
            "INSERT INTO search_events (user_id, library_id, kind, search_id, found_tracks, \
                                        found_albums, found_artists, created_at) \
             VALUES (7, ?1, iif(?3, 'searched', 'forgotten'), ?2, iif(?3, 1, NULL), \
                     iif(?3, 0, NULL), iif(?3, 0, NULL), ?4)",
            rusqlite::params![library, search, kept, at],
        )
    })
    .await
    .unwrap();
}

/// Each delivery's hook and library, and the IDs it named: the tracks played, the searches
/// found or forgotten, or zero for a request to forget everything.
fn named(received: Vec<Received>) -> Vec<(Hook, i64, Vec<u64>)> {
    let ids = |event: Option<Event>| match event {
        Some(Event::Played(plays)) => plays.iter().map(|p| p.track.id).collect(),
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
    let shared = db.write(|tx| set_search_sharing(tx, "plugin", 7, false, true)).await.unwrap();
    assert!(shared.is_ok());
    let since = now_ms();
    // The forget comes first, so a delivery reading every library would begin with it.
    noted(&db, 2, 2, false, since).await;
    noted(&db, 1, 1, true, since).await;
    played(&db, 1, 11, since).await;
    played(&db, 2, 21, since).await;
    let expected = [
        (Hook::Played, 1, vec![11]),
        (Hook::Played, 2, vec![21]),
        (Hook::Searched, 1, vec![1]),
        (Hook::Searched, 2, vec![2]),
    ];
    assert_eq!(named(deliver(&db, 0, HANDLED).await), expected);

    noted(&db, 2, 3, true, since).await;
    let expected = [(Hook::Searched, 2, vec![3])];
    assert_eq!(named(deliver(&db, 0, HANDLED).await), expected, "library 1 has nothing new");

    let stopped = db.write(|tx| set_search_sharing(tx, "plugin", 7, false, false)).await.unwrap();
    assert!(stopped.is_ok());
    let forget = [(Hook::Searched, 1, vec![0]), (Hook::Searched, 2, vec![0])];
    assert_eq!(named(deliver(&db, 0, HANDLED).await), forget, "in each library, once");
}
