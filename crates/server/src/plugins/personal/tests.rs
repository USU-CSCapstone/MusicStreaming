//! Connecting a plugin that acts for a user: who is offered it, how what they set is checked
//! and kept, and sharing their searches with it.

use jewelcase_plugins::Permission::{LibraryRead, SearchActivity, Searched};
use serde_json::{Map, Value, json};

use super::super::testing::{caller, connect, database, enable, grant, install, manifest, plugins};
use super::super::{PluginError, Plugins};
use super::{SharingError, connectable, disconnect, set_search_sharing, values};
use crate::db::testing::{User, access};
use crate::db::{Database, DbError};

/// `scrobbler`, which users connect with a token, and `lyrics`, which they set nothing for, are
/// both enabled in library 1, which user 7 reaches and user 8 does not.
async fn scrobbler(plugins: &Plugins, db: &Database) {
    let mut scrobbler = manifest("scrobbler", &[]);
    scrobbler["name"] = json!("Scrobbler");
    scrobbler["personalSettings"] = json!({
        "properties": { "token": { "type": "string", "writeOnly": true }, "user": { "type": "string" } },
        "required": ["token"]
    });
    install(db, scrobbler).await;
    install(db, manifest("lyrics", &[])).await;
    enable(plugins, "scrobbler", true).await;
    enable(plugins, "lyrics", true).await;
    db.write(|tx| {
        User::new(7, "user").insert(tx)?;
        User::new(8, "user").insert(tx)?;
        access(tx, 7, 1)?;
        access(tx, 8, 2)
    })
    .await
    .unwrap();
}

/// Each plugin `user` (an admin if `admin`) can connect, by name, and whether they connected it.
async fn listed(db: &Database, user: i64, admin: bool) -> Vec<(String, bool)> {
    let found = db.read(move |conn| connectable(conn, caller(user, admin))).await.unwrap();
    found.into_iter().map(|c| (c.name, c.connected)).collect()
}

#[tokio::test]
async fn a_plugin_is_offered_to_those_who_reach_a_library_it_is_enabled_in() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    scrobbler(&plugins, &db).await;
    let scrobbler = [("Scrobbler".to_owned(), false)];
    assert_eq!(listed(&db, 7, false).await, scrobbler, "not lyrics, with nothing to set");
    assert!(listed(&db, 8, false).await.is_empty(), "not in a library they reach");
    assert_eq!(listed(&db, 8, true).await, scrobbler, "an admin reaches every library");

    enable(&plugins, "scrobbler", false).await;
    assert!(listed(&db, 7, false).await.is_empty(), "not while it is disabled");
}

#[tokio::test]
async fn settings_are_checked_before_the_plugin_is_asked() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    scrobbler(&plugins, &db).await;
    let save = |user: i64, entered: Value| {
        let entered: Map<String, Value> = entered.as_object().unwrap().clone();
        plugins.save_personal("scrobbler".into(), caller(user, false), entered)
    };
    let refused = |result: Result<(), PluginError>| match result {
        Err(PluginError::SettingsInvalid(why)) => why,
        other => panic!("{other:?}"),
    };
    assert_eq!(refused(save(7, json!({ "user": "sam" })).await), "token is required.");
    assert_eq!(refused(save(7, json!({ "token": 5 })).await), "token must be text.");
    assert!(matches!(save(8, json!({ "token": "t" })).await, Err(PluginError::NotFound)));

    // A stored shared value of the same name, as an earlier version of the plugin declared,
    // gives none.
    let shared = "INSERT INTO plugin_settings (plugin_id, library_id, settings) \
                  VALUES ('scrobbler', 0, '{\"token\": \"stale\"}')";
    db.write(move |tx| tx.execute(shared, [])).await.unwrap();
    assert_eq!(refused(save(7, json!({ "user": "sam" })).await), "token is required.");
}

#[tokio::test]
async fn disconnecting_forgets_what_was_set() {
    let (temp, db) = database().await;
    scrobbler(&plugins(&temp, &db), &db).await;
    connect(&db, "scrobbler", 7, json!({ "token": "t" }), 0).await;
    assert_eq!(listed(&db, 7, false).await, [("Scrobbler".to_owned(), true)]);
    let stored = db.read(|conn| values(conn, "scrobbler", 7)).await.unwrap();
    assert_eq!(Value::Object(stored), json!({ "token": "t" }));

    db.write(|tx| disconnect(tx, "scrobbler", caller(7, false))).await.unwrap();
    assert_eq!(listed(&db, 7, false).await, [("Scrobbler".to_owned(), false)]);
    assert!(db.read(|conn| values(conn, "scrobbler", 7)).await.unwrap().is_empty());
}

/// Settings that cannot be read are an error rather than none, so a plugin never acts for a user
/// without what they set. SQLite takes a number too large for serde as JSON.
#[tokio::test]
async fn settings_that_cannot_be_read_are_an_error() {
    let (temp, db) = database().await;
    scrobbler(&plugins(&temp, &db), &db).await;
    connect(&db, "scrobbler", 7, json!({ "token": "t" }), 0).await;
    let unreadable = r#"{"token": "t", "limit": 1e400}"#;
    db.write(move |tx| tx.execute("UPDATE plugin_connections SET settings = ?1", [unreadable]))
        .await
        .unwrap();
    let read = db.read(|conn| values(conn, "scrobbler", 7)).await;
    assert!(
        matches!(read, Err(DbError::Sqlite(rusqlite::Error::FromSqlConversionFailure(0, ..)))),
        "{read:?}"
    );
}

/// `first` and `second`, which users connect with a token and may share their searches with, each
/// enabled in libraries 1 and 2; users 7 and 8 reach library 1.
async fn sharers(plugins: &Plugins, db: &Database) {
    for id in ["first", "second"] {
        let mut sharer = manifest(id, &[LibraryRead, SearchActivity, Searched]);
        sharer["personalSettings"] = json!({ "properties": { "token": { "type": "string" } } });
        install(db, sharer).await;
        grant(plugins, id, &[SearchActivity, Searched], &[LibraryRead]).await;
        for library in [1, 2] {
            plugins.set_enabled(id.into(), library, true).await.unwrap();
        }
    }
    db.write(|tx| {
        for user in [7, 8] {
            User::new(user, "user").insert(tx)?;
            access(tx, user, 1)?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

/// Each plugin `user` can connect, by ID: whether they connected it, and whether they share
/// their searches with it.
async fn connections(db: &Database, user: i64) -> Vec<(String, bool, bool)> {
    let found = db.read(move |conn| connectable(conn, caller(user, false))).await.unwrap();
    found.into_iter().map(|c| (c.id, c.connected, c.shares_searches)).collect()
}

/// Turns sharing `user`'s searches with `id` on or off, as that user rather than an admin.
async fn share(db: &Database, id: &'static str, user: i64, on: bool) -> Result<(), SharingError> {
    db.try_write(move |tx| set_search_sharing(tx, id, caller(user, false), on)).await
}

/// Every request to forget written, as (user, library, plugin).
async fn stopped(db: &Database) -> Vec<(i64, i64, String)> {
    db.read(|conn| {
        conn.prepare(
            "SELECT user_id, library_id, plugin_id FROM search_events WHERE kind = 'stopped' \
             ORDER BY seq",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect()
    })
    .await
    .unwrap()
}

/// A row of [`connections`].
fn row(id: &str, connected: bool, shares: bool) -> (String, bool, bool) {
    (id.to_owned(), connected, shares)
}

#[tokio::test]
async fn connecting_shares_nothing_and_sharing_needs_a_connection() {
    let (temp, db) = database().await;
    sharers(&plugins(&temp, &db), &db).await;
    assert!(matches!(share(&db, "first", 7, true).await, Err(SharingError::Unconnected)));
    let nothing = [row("first", false, false), row("second", false, false)];
    assert_eq!(connections(&db, 7).await, nothing, "no sharing without settings");

    connect(&db, "first", 7, json!({ "token": "t" }), 100).await;
    assert_eq!(connections(&db, 7).await, [row("first", true, false), row("second", false, false)]);
    db.write(|tx| disconnect(tx, "first", caller(7, false))).await.unwrap();
    assert_eq!(connections(&db, 7).await, nothing);
    assert!(stopped(&db).await.is_empty(), "nothing to forget when they never shared");
}

#[tokio::test]
async fn sharing_is_per_user_and_per_plugin_and_holds_in_every_library() {
    let (temp, db) = database().await;
    sharers(&plugins(&temp, &db), &db).await;
    for (id, user) in [("first", 7), ("second", 7), ("first", 8), ("second", 8)] {
        connect(&db, id, user, json!({ "token": "t" }), 100).await;
    }
    assert!(share(&db, "first", 7, true).await.is_ok());
    assert_eq!(connections(&db, 7).await, [row("first", true, true), row("second", true, false)]);
    assert_eq!(connections(&db, 8).await, [row("first", true, false), row("second", true, false)]);

    assert!(share(&db, "second", 7, true).await.is_ok());
    assert!(share(&db, "first", 8, true).await.is_ok());
    assert!(share(&db, "first", 7, false).await.is_ok());
    assert_eq!(connections(&db, 7).await, [row("first", true, false), row("second", true, true)]);
    assert_eq!(connections(&db, 8).await, [row("first", true, true), row("second", true, false)]);
    let forget = vec![(7, 1, "first".to_owned()), (7, 2, "first".to_owned())];
    assert_eq!(stopped(&db).await, forget, "asked to forget wherever it serves, reached or not");
    assert!(share(&db, "first", 7, false).await.is_ok());
    assert_eq!(stopped(&db).await, forget, "once");
}

#[tokio::test]
async fn disconnecting_while_sharing_stops_sharing_and_asks_to_forget() {
    let (temp, db) = database().await;
    sharers(&plugins(&temp, &db), &db).await;
    for user in [7, 8] {
        connect(&db, "first", user, json!({ "token": "t" }), 100).await;
        assert!(share(&db, "first", user, true).await.is_ok());
    }
    db.write(|tx| disconnect(tx, "first", caller(7, false))).await.unwrap();
    assert_eq!(
        connections(&db, 7).await,
        [row("first", false, false), row("second", false, false)]
    );
    assert!(db.read(|conn| values(conn, "first", 7)).await.unwrap().is_empty());
    let forget = [(7, 1, "first".to_owned()), (7, 2, "first".to_owned())];
    assert_eq!(stopped(&db).await, forget, "wherever it serves, reached or not");
    assert_eq!(connections(&db, 8).await, [row("first", true, true), row("second", false, false)]);
}

#[tokio::test]
async fn saving_settings_again_keeps_when_they_connected_and_their_sharing() {
    let (temp, db) = database().await;
    sharers(&plugins(&temp, &db), &db).await;
    connect(&db, "first", 7, json!({ "token": "t" }), 100).await;
    assert!(share(&db, "first", 7, true).await.is_ok());
    connect(&db, "first", 7, json!({ "token": "u" }), 500).await;
    let stored = db.read(|conn| values(conn, "first", 7)).await.unwrap();
    assert_eq!(Value::Object(stored), json!({ "token": "u" }));
    assert_eq!(connections(&db, 7).await[0], row("first", true, true));
    let connected_at: i64 = db
        .read(|conn| {
            conn.query_row("SELECT connected_at FROM plugin_connections", [], |row| row.get(0))
        })
        .await
        .unwrap();
    assert_eq!(connected_at, 100, "listening from before stays unsent");
}

#[tokio::test]
async fn what_a_user_set_is_read_only_for_that_plugin_and_that_user() {
    let (temp, db) = database().await;
    sharers(&plugins(&temp, &db), &db).await;
    for (id, user) in [("first", 7), ("second", 7), ("first", 8)] {
        connect(&db, id, user, json!({ "token": format!("{id} {user}") }), 100).await;
    }
    for (id, user, expected) in [
        ("first", 7, json!({ "token": "first 7" })),
        ("second", 7, json!({ "token": "second 7" })),
        ("first", 8, json!({ "token": "first 8" })),
        ("second", 8, json!({})),
    ] {
        let stored = db.read(move |conn| values(conn, id, user)).await.unwrap();
        assert_eq!(Value::Object(stored), expected, "{id} for user {user}");
    }
}

#[tokio::test]
async fn connecting_to_a_plugin_no_longer_installed_connects_nothing() {
    let (temp, db) = database().await;
    scrobbler(&plugins(&temp, &db), &db).await;
    let connected = db.try_write(|tx| super::connect(tx, "gone", 7, &Map::new(), 0)).await;
    assert!(matches!(connected, Err(PluginError::NotFound)), "{connected:?}");
    let count: i64 = db
        .read(|conn| conn.query_row("SELECT count(*) FROM plugin_connections", [], |r| r.get(0)))
        .await
        .unwrap();
    assert_eq!(count, 0);
}
