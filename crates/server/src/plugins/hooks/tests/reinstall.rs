//! Uninstalling a plugin and installing it again: its users' connections go with it, so nothing
//! they did before is sent again, and they reconnect to be acted for.

use jewelcase_plugins::Manifest;
use jewelcase_plugins::Permission::{
    LibraryRead, ListeningActivity, Played, SearchActivity, Searched,
};
use serde_json::{Value, json};

use super::{HANDLED, connected, deliver, play, setup, tracks_by_user};
use crate::db::Database;
use crate::plugins::personal::set_search_sharing;
use crate::plugins::testing::{caller, connect, enable, grant, install, manifest};
use crate::plugins::{PluginError, add};

/// How many rows `table` has.
async fn count(db: &Database, table: &'static str) -> i64 {
    let sql = format!("SELECT count(*) FROM {table}");
    db.read(move |conn| conn.query_row(&sql, [], |row| row.get(0))).await.unwrap()
}

/// The manifest `plugin` was installed with.
async fn installed(db: &Database) -> Value {
    let json: String = db
        .read(|conn| {
            conn.query_row("SELECT manifest FROM plugins WHERE id = 'plugin'", [], |row| row.get(0))
        })
        .await
        .unwrap();
    serde_json::from_str(&json).unwrap()
}

#[tokio::test]
async fn a_reinstall_sends_nothing_from_before_and_users_reconnect() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Played, SearchActivity, Searched]).await;
    db.try_write(|tx| set_search_sharing(tx, "plugin", caller(7, false), true)).await.unwrap();
    play(&db, "a", 7, 11, 200, "finished").await;
    play(&db, "b", 7, 12, 210, "finished").await;
    assert_eq!(tracks_by_user(deliver(&db, 0, HANDLED).await), [(7, vec![11, 12])]);

    let manifest = installed(&db).await;
    plugins.uninstall("plugin".into()).await.unwrap();
    assert_eq!(count(&db, "plugin_connections").await, 0, "its users' connections go with it");
    assert_eq!(count(&db, "plugin_cursors").await, 0, "and so does how far it was sent");
    assert_eq!(count(&db, "search_events").await, 0, "with no plugin left to tell to forget");

    install(&db, manifest).await;
    enable(&plugins, "plugin", true).await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "nothing is due again");
    connect(&db, "plugin", 7, json!({}), 1000).await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "nor once they reconnect");
    play(&db, "c", 7, 11, 1100, "finished").await;
    let delivered = tracks_by_user(deliver(&db, 0, HANDLED).await);
    assert_eq!(delivered, [(7, vec![11])], "only what they played since, with its grants kept");
}

#[tokio::test]
async fn installing_it_while_it_is_installed_changes_nothing() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[ListeningActivity, Played]).await;
    play(&db, "a", 7, 11, 200, "finished").await;
    assert_eq!(tracks_by_user(deliver(&db, 0, HANDLED).await), [(7, vec![11])]);

    let mut asks_less: Manifest = serde_json::from_value(installed(&db).await).unwrap();
    asks_less.permissions.clear();
    let added = db.try_write(move |tx| add(tx, &asks_less, None)).await;
    assert!(matches!(added, Err(PluginError::Exists(_))), "already installed");
    assert_eq!(count(&db, "plugin_connections").await, 1, "its users stay connected");
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "nothing is sent again");
    play(&db, "b", 7, 12, 300, "finished").await;
    let delivered = tracks_by_user(deliver(&db, 0, HANDLED).await);
    assert_eq!(delivered, [(7, vec![12])], "and its grants are kept");
}

#[tokio::test]
async fn a_reinstall_is_not_told_again_to_forget_what_it_was_told_before() {
    let (_temp, db, plugins) = setup().await;
    connected(&plugins, &db, &[SearchActivity, Searched]).await;
    for sharing in [true, false] {
        db.try_write(move |tx| set_search_sharing(tx, "plugin", caller(7, false), sharing))
            .await
            .unwrap();
    }
    assert_eq!(deliver(&db, 0, HANDLED).await.len(), 1, "told to forget once");
    // User 7 stops sharing with `other` too, which is then disabled before it is told.
    let mut other = manifest("other", &[LibraryRead, SearchActivity, Searched]);
    other["personalSettings"] = json!({ "properties": { "token": { "type": "string" } } });
    install(&db, other).await;
    grant(&plugins, "other", &[SearchActivity, Searched], &[LibraryRead]).await;
    enable(&plugins, "other", true).await;
    connect(&db, "other", 7, json!({}), 0).await;
    for sharing in [true, false] {
        db.try_write(move |tx| set_search_sharing(tx, "other", caller(7, false), sharing))
            .await
            .unwrap();
    }
    enable(&plugins, "other", false).await;

    let manifest = installed(&db).await;
    plugins.uninstall("plugin".into()).await.unwrap();
    assert_eq!(count(&db, "search_events").await, 1, "the stop meant for `other` is kept");
    install(&db, manifest).await;
    enable(&plugins, "plugin", true).await;
    assert!(deliver(&db, 0, HANDLED).await.is_empty(), "and never again");
}
