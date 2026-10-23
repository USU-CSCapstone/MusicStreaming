//! A user's settings check runs in a turn of that user's, taken in the plugin's turn at the
//! server level, and uninstalling, which holds that turn, waits for every user's.

use jewelcase_plugins::Manifest;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use super::super::install::add;
use super::super::settings::SERVER;
use super::super::testing::caller;
use super::turns::{installed, queued, waits};
use super::*;
use crate::db::testing::User;

/// Installs the plugin, which users connect with a token, enabled in library 1, with user 7, an
/// admin, to connect it. Answers its manifest, and makes its file a FIFO ([`gate`]).
async fn personal(plugins: &Plugins, db: &Database) -> Value {
    let mut asks = manifest("plugin", &[]);
    asks["personalSettings"] = json!({ "properties": { "token": { "type": "string" } } });
    install(db, asks.clone()).await;
    enable(plugins, "plugin", true).await;
    db.write(|tx| User::new(7, "admin").insert(tx)).await.unwrap();
    let file = plugins.path("plugin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    gate(&file);
    asks
}

/// Starts user 7 connecting the plugin with a token, which the plugin checks.
fn check(plugins: &Arc<Plugins>) -> JoinHandle<Result<(), PluginError>> {
    let plugins = plugins.clone();
    let entered = json!({ "token": "t" }).as_object().unwrap().clone();
    tokio::spawn(
        async move { plugins.save_personal("plugin".into(), caller(7, true), entered).await },
    )
}

/// Starts uninstalling the plugin.
fn uninstall(plugins: &Arc<Plugins>) -> JoinHandle<Result<(), PluginError>> {
    let plugins = plugins.clone();
    tokio::spawn(async move { plugins.uninstall("plugin".into()).await })
}

/// Waits until `count` hold or wait for user 7's turn. The deadline only bounds a hang.
async fn queued_for_user(plugins: &Plugins, count: usize) {
    let queued = async {
        while plugins.turns.waiting_personal("plugin", 7) < count {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(Duration::from_secs(30), queued).await.expect("queued within 30 s");
}

/// How many users have connected the plugin.
async fn connections(db: &Database) -> i64 {
    let sql = "SELECT count(*) FROM plugin_connections";
    db.read(move |conn| conn.query_row(sql, [], |row| row.get(0))).await.unwrap()
}

/// A user's check holds that user's turn, leaving other users' and the server level's free, and
/// uninstalling waits for it to end before it removes anything.
#[tokio::test]
async fn a_personal_check_holds_its_own_turn_and_uninstalling_waits_for_it() {
    let (_temp, db, plugins, thread) = scanned().await;
    personal(&plugins, &db).await;
    let check = check(&plugins);
    let loading = loading(plugins.path("plugin")).await;

    assert!(waits(plugins.turns.take_personal("plugin", 7)), "its user's turn is taken");
    assert!(!waits(plugins.turns.take_personal("plugin", 8)), "another user's is not");
    assert!(!waits(plugins.turns.take("plugin", SERVER)), "nor the server level's");
    let uninstall = uninstall(&plugins);
    queued_for_user(&plugins, 2).await;
    assert!(installed(&db).await, "uninstalling waits for the check");

    let_in(loading);
    uninstall.await.unwrap().unwrap();
    assert!(!installed(&db).await);
    // Saved before the uninstall, then gone with it, or refused after it.
    let checked = check.await.unwrap();
    assert!(matches!(checked, Ok(()) | Err(PluginError::NotFound)), "{checked:?}");
    assert_eq!(connections(&db).await, 0);
    stop(thread).await;
}

/// Uninstalling holds the plugin's turn at the server level, so a user's check queued behind it
/// finds the plugin gone: it runs nothing, answers that there is no such plugin, and connects
/// nothing.
#[tokio::test]
async fn a_personal_check_queued_behind_an_uninstall_runs_nothing() {
    let (_temp, db, plugins, thread) = scanned().await;
    personal(&plugins, &db).await;
    let turn = plugins.turns.take("plugin", 1).await;
    let uninstall = uninstall(&plugins);
    queued(&plugins, 1, 2).await;
    let check = check(&plugins);
    queued(&plugins, SERVER, 2).await;

    drop(turn);
    uninstall.await.unwrap().unwrap();
    // A check that loaded the plugin would wait at the FIFO, or fail once it is removed.
    let checked = tokio::time::timeout(Duration::from_secs(30), check).await;
    let checked = checked.expect("the check ends within 30 s").unwrap();
    assert!(matches!(checked, Err(PluginError::NotFound)), "{checked:?}");
    assert_eq!(connections(&db).await, 0);
    stop(thread).await;
}

/// While uninstalling waits for a user's check, the plugin is still installed, so installing it
/// again is refused rather than put in place for the uninstall to remove; installed again once
/// the uninstall returns, it keeps its file.
#[tokio::test]
async fn a_reinstall_while_uninstalling_waits_keeps_its_file() {
    let (_temp, db, plugins, thread) = scanned().await;
    let asks = personal(&plugins, &db).await;
    let manifest: Manifest = serde_json::from_value(asks).unwrap();
    let file = plugins.path("plugin");
    let check = check(&plugins);
    let loading = loading(file.clone()).await;
    let uninstall = uninstall(&plugins);
    queued_for_user(&plugins, 2).await;

    // As `Plugins::install` does: its row, and then its file put in place.
    let (db_ref, file_ref) = (&db, &file);
    let reinstall = move |manifest: Manifest| async move {
        let (db, file) = (db_ref, file_ref);
        let added = db.try_write(move |tx| add(tx, &manifest, None)).await;
        if added.is_ok() {
            let staged = file.with_extension("staged");
            std::fs::write(&staged, b"the new version").unwrap();
            std::fs::rename(&staged, file).unwrap();
        }
        added
    };
    let early = reinstall(manifest.clone()).await;
    assert!(matches!(early, Err(PluginError::Exists(_))), "{early:?}");

    let_in(loading);
    uninstall.await.unwrap().unwrap();
    let checked = check.await.unwrap();
    assert!(matches!(checked, Ok(()) | Err(PluginError::NotFound)), "{checked:?}");
    reinstall(manifest).await.unwrap();
    assert!(installed(&db).await, "reinstalled");
    assert!(file.exists(), "the reinstalled plugin's file was removed by the earlier uninstall");
    stop(thread).await;
}
