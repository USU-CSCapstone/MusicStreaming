use serde_json::{Map, Value, json};

use super::super::testing::{connect, database, enable, install, manifest, plugins};
use super::super::{PluginError, Plugins};
use super::{connectable, disconnect, values};
use crate::db::Database;
use crate::db::testing::{User, access};

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

async fn listed(db: &Database, user: i64, admin: bool) -> Vec<(String, bool)> {
    let found = db.read(move |conn| connectable(conn, user, admin)).await.unwrap();
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
        plugins.save_personal("scrobbler".into(), user, false, entered)
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
    let (temp, db) = database().await;
    scrobbler(&plugins(&temp, &db), &db).await;
    connect(&db, "scrobbler", 7, json!({ "token": "t" }), 0).await;
    assert_eq!(listed(&db, 7, false).await, [("Scrobbler".to_owned(), true)]);
    let stored = db.read(|conn| values(conn, "scrobbler", 7)).await.unwrap();
    assert_eq!(Value::Object(stored), json!({ "token": "t" }));

    db.write(|tx| disconnect(tx, "scrobbler", 7)).await.unwrap();
    assert_eq!(listed(&db, 7, false).await, [("Scrobbler".to_owned(), false)]);
    assert!(db.read(|conn| values(conn, "scrobbler", 7)).await.unwrap().is_empty());
}
