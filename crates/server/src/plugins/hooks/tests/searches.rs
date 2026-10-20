//! The `searched` hook, which tells a plugin the searches of the users who share them with it.

use jewelcase_plugins::Event;
use jewelcase_plugins::Permission::{LibraryRead, SearchActivity, Searched};

use super::{HANDLED, connected, deliver, one, setup};
use crate::db::{Database, now_ms};
use crate::plugins::Plugins;
use crate::plugins::personal::{SharingRefused, disconnect, set_search_sharing};
use crate::plugins::testing::grant;

/// `plugin` granted the searched hook and what it needs, and connected by user 7, who shares their
/// searches with it from now; user 8 shares nothing. Returns a time since they began sharing.
async fn sharing(plugins: &Plugins, db: &Database) -> i64 {
    connected(plugins, db, &[SearchActivity, Searched]).await;
    let shared = db.write(|tx| set_search_sharing(tx, "plugin", 7, false, true)).await.unwrap();
    assert!(shared.is_ok());
    now_ms()
}

/// User `user` settles on search `id` for `query` at `at`, which matched `tracks` tracks.
async fn searched(db: &Database, id: i64, user: i64, query: &'static str, at: i64, tracks: i64) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO recent_searches (id, user_id, library_id, query, folded, searched_at, \
                                          found_tracks, selected_type, selected_id) \
             VALUES (?1, ?2, 1, ?3, ?3, ?4, ?5, 'track', 11)",
            rusqlite::params![id, user, query, at, tracks],
        )?;
        tx.execute(
            "INSERT INTO search_events (user_id, library_id, kind, search_id, created_at) \
             VALUES (?1, 1, 'searched', ?2, ?3)",
            rusqlite::params![user, id, at],
        )
    })
    .await
    .unwrap();
}

/// User 7 removes search `id` at `at`.
async fn forgot(db: &Database, id: i64, at: i64) {
    db.write(move |tx| {
        tx.execute("DELETE FROM recent_searches WHERE id = ?1", [id])?;
        tx.execute(
            "INSERT INTO search_events (user_id, library_id, kind, search_id, created_at) \
             VALUES (7, 1, 'forgotten', ?1, ?2)",
            rusqlite::params![id, at],
        )
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn searches_reach_a_plugin_only_from_users_who_share_them_and_only_since() {
    let (_temp, db, plugins) = setup().await;
    let since = sharing(&plugins, &db).await;
    searched(&db, 1, 7, "before", 0, 2).await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "searched before they shared");

    searched(&db, 2, 7, "a song they do not own", since, 0).await;
    searched(&db, 3, 8, "not shared", since, 1).await;
    let Event::Searched(found) = one(deliver(&db, 0, HANDLED).await, 7) else {
        panic!("not searches")
    };
    let summary: Vec<_> = found.iter().map(|s| (s.id, s.query.as_str(), s.found.tracks)).collect();
    assert_eq!(summary, [(2, "a song they do not own", 0)], "theirs; nothing found is still sent");
    assert_eq!(found[0].selected.map(|s| s.id), Some(11));
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "each search is delivered once");
}

#[tokio::test]
async fn a_removed_search_is_never_sent_and_a_forget_follows_what_it_names() {
    let (_temp, db, plugins) = setup().await;
    let since = sharing(&plugins, &db).await;
    searched(&db, 2, 7, "kept", since, 1).await;
    searched(&db, 3, 7, "removed", since, 1).await;
    forgot(&db, 3, since).await;

    let Event::Searched(found) = one(deliver(&db, 0, HANDLED).await, 7) else {
        panic!("not searches")
    };
    assert_eq!(found.iter().map(|s| s.id).collect::<Vec<_>>(), [2], "removed before it was sent");
    let forgotten = one(deliver(&db, 0, HANDLED).await, 7);
    assert!(
        matches!(forgotten, Event::SearchesForgotten(Some(ref ids)) if ids == &[3]),
        "{forgotten:?}"
    );
}

#[tokio::test]
async fn stopping_asks_the_plugin_to_forget_everything_even_once_disconnected() {
    let (_temp, db, plugins) = setup().await;
    let since = sharing(&plugins, &db).await;
    searched(&db, 2, 7, "kept", since, 1).await;
    db.write(|tx| disconnect(tx, "plugin", 7)).await.unwrap();
    let shared: i64 = db
        .read(|conn| {
            conn.query_row("SELECT count(*) FROM plugin_search_sharing", [], |row| row.get(0))
        })
        .await
        .unwrap();
    assert_eq!(shared, 0, "disconnecting stops sharing");

    let told = one(deliver(&db, 0, HANDLED).await, 7);
    assert!(
        matches!(told, Event::SearchesForgotten(None)),
        "told, though no longer connected, to forget everything, and not sent the search: {told:?}"
    );
}

#[tokio::test]
async fn searches_need_search_activity_as_well_as_the_hook() {
    let (_temp, db, plugins) = setup().await;
    let since = sharing(&plugins, &db).await;
    grant(&plugins, "plugin", &[Searched], &[LibraryRead]).await;
    searched(&db, 2, 7, "kept", since, 1).await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty());
}

#[tokio::test]
async fn sharing_is_turned_on_only_by_a_connected_user_of_a_plugin_allowed_searches() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[]).await;
    let set = |user: i64, sharing: bool| {
        let db = db.clone();
        async move {
            db.write(move |tx| set_search_sharing(tx, "plugin", user, true, sharing)).await.unwrap()
        }
    };
    assert!(matches!(set(8, true).await, Err(SharingRefused::Unconnected)));
    assert!(
        matches!(set(7, true).await, Err(SharingRefused::SearchesNotAllowed)),
        "not granted searches"
    );
    grant(&plugins, "plugin", &[Searched], &[LibraryRead]).await;
    assert!(
        matches!(set(7, true).await, Err(SharingRefused::SearchesNotAllowed)),
        "not granted search activity"
    );
    grant(&plugins, "plugin", &[SearchActivity, Searched], &[LibraryRead]).await;
    assert!(set(7, true).await.is_ok());
    assert!(set(7, true).await.is_ok(), "turning it on twice keeps when it was first turned on");
    assert!(set(7, false).await.is_ok());
    let stopped: i64 = db
        .read(|conn| {
            conn.query_row("SELECT count(*) FROM search_events WHERE kind = 'stopped'", [], |row| {
                row.get(0)
            })
        })
        .await
        .unwrap();
    assert_eq!(stopped, 1, "one request to forget, in the one library it serves");
}
