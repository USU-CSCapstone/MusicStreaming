//! What each scope of grant reaches: a plugin's own grants hold in every library, and a
//! library's hold only there.

use std::sync::Arc;

use jewelcase_plugins::Permission::{self, LibraryAdd, LibraryRead, Network, Schedule};
use serde_json::{Value, json};

use super::super::Plugins;
use super::super::testing::{database, install, manifest, plugins};
use super::granted;
use crate::db::testing::{library, stored};
use crate::db::{Database, Text};

/// A manifest for `plugin` that asks for each of `permissions`, the network to any host and a
/// schedule of an hour.
fn asking(permissions: &[Permission]) -> Value {
    let mut manifest = manifest("plugin", permissions);
    for request in manifest["permissions"].as_array_mut().unwrap() {
        match request["permission"].as_str() {
            Some("network") => request["destinations"] = json!(["*"]),
            Some("schedule") => request["everyMinutes"] = json!(60),
            _ => {}
        }
    }
    manifest
}

/// The catalog of [`database`] with `plugin` installed, asking for the network, a schedule, and
/// to read and add to libraries, and granted none of them.
async fn setup() -> (tempfile::TempDir, Arc<Database>, Arc<Plugins>) {
    let (temp, db) = database().await;
    install(&db, asking(&[Network, Schedule, LibraryRead, LibraryAdd])).await;
    let plugins = plugins(&temp, &db);
    (temp, db, plugins)
}

/// Replaces what `plugin` is granted, as the admin does.
async fn set(plugins: &Plugins, plugin_wide: &[Permission], per_library: &[(i64, &[Permission])]) {
    let per_library = per_library.iter().map(|(l, p)| (*l, p.to_vec())).collect();
    plugins.set_grants("plugin".into(), plugin_wide.to_vec(), per_library).await.unwrap();
}

/// What `plugin` holds in `library`, by name and sorted.
async fn held(db: &Database, library: i64) -> Vec<String> {
    let permissions = db.read(move |conn| granted(conn, "plugin", library)).await.unwrap();
    let mut names: Vec<_> = permissions.into_iter().map(Permission::name).collect();
    names.sort_unstable();
    names
}

#[tokio::test]
async fn a_plugins_own_grants_hold_everywhere_and_a_librarys_only_there() {
    let (_temp, db, plugins) = setup().await;
    // Each scope is also offered a permission of the other, which it ignores.
    set(&plugins, &[Network, LibraryRead], &[(1, &[LibraryAdd, Schedule])]).await;
    assert_eq!(held(&db, 1).await, ["libraryAdd", "network"]);
    assert_eq!(held(&db, 2).await, ["network"], "library 1's grant does not hold in library 2");
    assert_eq!(held(&db, 0).await, ["network"], "only the plugin's own");
}

#[tokio::test]
async fn replacing_grants_leaves_the_scopes_not_named() {
    let (_temp, db, plugins) = setup().await;
    set(&plugins, &[Network], &[(1, &[LibraryAdd]), (2, &[LibraryRead])]).await;
    set(&plugins, &[], &[(2, &[])]).await;
    assert_eq!(held(&db, 1).await, ["libraryAdd"], "library 1 was not named");
    assert!(held(&db, 2).await.is_empty());

    set(&plugins, &[Network], &[(0, &[LibraryRead]), (9, &[LibraryRead])]).await;
    assert_eq!(held(&db, 0).await, ["network"], "library 0 names no library");
    assert_eq!(held(&db, 9).await, ["network"], "nor does one that does not exist");
    assert_eq!(held(&db, 1).await, ["libraryAdd", "network"]);
}

#[tokio::test]
async fn a_deleted_librarys_grants_and_settings_go_with_it() {
    let (_temp, db, plugins) = setup().await;
    db.write(|tx| {
        library(tx, 3, "Third", "/third")?;
        tx.execute_batch(
            "INSERT INTO plugin_settings (plugin_id, library_id, settings) \
             VALUES ('plugin', 0, '{}'), ('plugin', 3, '{}')",
        )
    })
    .await
    .unwrap();
    set(&plugins, &[Network], &[(3, &[LibraryAdd])]).await;
    db.write(|tx| {
        tx.execute("DELETE FROM libraries WHERE id = 3", [])?;
        library(tx, 3, "New", "/new")
    })
    .await
    .unwrap();
    assert_eq!(held(&db, 3).await, ["network"], "a new library given its ID has none of them");
    let settings: Vec<i64> = db
        .read(|conn| {
            conn.prepare("SELECT library_id FROM plugin_settings")?
                .query_map([], |row| row.get(0))?
                .collect()
        })
        .await
        .unwrap();
    assert_eq!(settings, [0], "only the settings of every library are left");
}

/// The manifest of [`setup`]'s `plugin`, asking for the network only at `destinations`, at most
/// once a second at `lrclib.net`, for a reason that JSON must escape.
fn reaching(destinations: &[&str]) -> Value {
    let mut manifest = asking(&[Network, Schedule, LibraryRead, LibraryAdd]);
    manifest["permissions"][0]["destinations"] = json!(destinations);
    manifest["permissions"][0]["reason"] = json!("To fetch \"synced\" lyrics\n— für \\ dich.");
    manifest["permissions"][0]["rateLimits"] = json!({ "lrclib.net": "1/s" });
    manifest
}

/// Uninstalls `plugin` and installs it again as `manifest` describes.
async fn reinstall(db: &Database, plugins: &Plugins, manifest: Value) {
    plugins.uninstall("plugin".into()).await.unwrap();
    install(db, manifest).await;
}

#[tokio::test]
async fn a_reinstall_keeps_only_the_grants_it_asks_for_again() {
    let (_temp, db, plugins) = setup().await;
    set(&plugins, &[Network, Schedule], &[(1, &[LibraryRead, LibraryAdd])]).await;
    reinstall(&db, &plugins, asking(&[Schedule, LibraryAdd])).await;
    assert_eq!(held(&db, 1).await, ["libraryAdd", "schedule"]);
}

#[tokio::test]
async fn a_reinstall_keeps_a_grant_only_for_the_same_request() {
    let (temp, db) = database().await;
    install(&db, reaching(&["lrclib.net"])).await;
    let plugins = plugins(&temp, &db);
    set(&plugins, &[Network, Schedule], &[(1, &[LibraryRead, LibraryAdd])]).await;
    reinstall(&db, &plugins, reaching(&["lrclib.net"])).await;
    let all = ["libraryAdd", "libraryRead", "network", "schedule"];
    assert_eq!(held(&db, 1).await, all, "each asked for exactly as approved");

    let mut changed = reaching(&["lrclib.net"]);
    changed["permissions"][0]["rateLimits"] = json!({ "lrclib.net": "2/s" });
    changed["permissions"][3]["required"] = json!(true);
    reinstall(&db, &plugins, changed.clone()).await;
    assert_eq!(held(&db, 1).await, ["libraryRead", "schedule"], "another limit, or now required");

    set(&plugins, &[Network, Schedule], &[(1, &[LibraryRead, LibraryAdd])]).await;
    changed["permissions"][0]["destinations"] = json!(["*"]);
    changed["permissions"][1]["everyMinutes"] = json!(5);
    changed["permissions"][2]["reason"] = json!("To read your tags.");
    reinstall(&db, &plugins, changed).await;
    assert_eq!(held(&db, 1).await, ["libraryAdd"], "any host, more often, or for another reason");
}

/// Each permission is stored by its name in manifests and the API, which the shared manifest
/// cases hold to.
#[test]
fn every_permission_is_stored_by_its_manifest_name() {
    for permission in Permission::ALL {
        assert_eq!(stored(Text(permission)), permission.name());
    }
}
