use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::super::testing::database;
use super::super::{PluginError, Plugins};
use super::{connectable, disconnect, values};
use crate::db::Database;

/// `p` has personal settings and is enabled in library 1, which user 7 reaches and user 8
/// does not.
async fn scrobbler(db: &Database) {
    db.write(|tx| {
        tx.execute_batch(
            r#"UPDATE plugins SET manifest = '{"id":"p","name":"Scrobbler","version":"1","apiVersion":"0.2","permissions":[],"personalSettings":{"properties":{"token":{"type":"string","writeOnly":true},"user":{"type":"string"}},"required":["token"]}}';
               INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES ('p', 1, 1);
               INSERT INTO users (id, username, display_name, role, password, created_at, updated_at)
               VALUES (7, 'seven', 'Seven', 'user', '', 0, 0), (8, 'eight', 'Eight', 'user', '', 0, 0);
               INSERT INTO library_access VALUES (7, 1, 0), (8, 2, 0);"#,
        )
    })
    .await
    .unwrap();
}

async fn listed(db: &Database, user: i64, admin: bool) -> Vec<(String, bool)> {
    let found = db.read(move |conn| connectable(conn, user, admin)).await.unwrap();
    found.into_iter().map(|c| (c.name, c.connected)).collect()
}

#[tokio::test]
async fn a_plugin_is_offered_to_those_who_reach_a_library_it_is_enabled_in() {
    let (_temp, db) = database().await;
    scrobbler(&db).await;
    assert_eq!(listed(&db, 7, false).await, [("Scrobbler".to_owned(), false)]);
    assert!(listed(&db, 8, false).await.is_empty(), "not in a library they reach");
    assert_eq!(listed(&db, 8, true).await.len(), 1, "an admin reaches every library");

    db.write(|tx| tx.execute_batch("UPDATE plugin_libraries SET enabled = 0")).await.unwrap();
    assert!(listed(&db, 7, false).await.is_empty(), "not while it is disabled");
    db.write(|tx| {
        tx.execute_batch(
            r#"UPDATE plugin_libraries SET enabled = 1; UPDATE plugins SET manifest = '{"name":"Lyrics"}'"#,
        )
    })
    .await
    .unwrap();
    assert!(listed(&db, 7, false).await.is_empty(), "nothing for a user to set");
}

#[tokio::test]
async fn settings_are_checked_before_the_plugin_is_asked() {
    let (temp, db) = database().await;
    scrobbler(&db).await;
    let plugins = Plugins::new(db.clone(), temp.path().join("plugins"), HashMap::new());
    let save = |user: i64, entered: Value| {
        let entered: Map<String, Value> = entered.as_object().unwrap().clone();
        plugins.save_personal("p".into(), user, false, entered)
    };
    let refused = |result: Result<(), PluginError>| match result {
        Err(PluginError::SettingsInvalid(why)) => why,
        other => panic!("{other:?}"),
    };
    assert_eq!(refused(save(7, json!({ "user": "sam" })).await), "token is required.");
    assert_eq!(refused(save(7, json!({ "token": 5 })).await), "token must be text.");
    assert!(matches!(save(8, json!({ "token": "t" })).await, Err(PluginError::NotFound)));
}

#[tokio::test]
async fn disconnecting_forgets_what_was_set() {
    let (_temp, db) = database().await;
    scrobbler(&db).await;
    db.write(|tx| {
        tx.execute_batch(r#"INSERT INTO plugin_user_settings VALUES ('p', 7, '{"token":"t"}', 0)"#)
    })
    .await
    .unwrap();
    assert_eq!(listed(&db, 7, false).await, [("Scrobbler".to_owned(), true)]);
    let stored = db.read(|conn| values(conn, "p", 7)).await.unwrap();
    assert_eq!(Value::Object(stored), json!({ "token": "t" }));

    db.write(|tx| disconnect(tx, "p", 7)).await.unwrap();
    assert_eq!(listed(&db, 7, false).await, [("Scrobbler".to_owned(), false)]);
    assert!(db.read(|conn| values(conn, "p", 7)).await.unwrap().is_empty());
}
