//! What the plugins' tests share: a small catalog, plugins installed with real manifests and
//! granted and enabled as an admin does it, and the steps only a running plugin can take.

use std::collections::HashMap;
use std::sync::Arc;

use jewelcase_plugins::{Manifest, Permission, manifest};
use serde_json::{Value, json};

use super::{Plugins, personal};
use crate::db::Database;
use crate::db::testing::{Album, Artist, Track, album_artist, library, track_artist};

/// Library 1 holds tracks 11, 12, and 13 (13 missing since a scan) on album 101, "Signal", by
/// artist 301, Aurora Lane, who is credited on track 11; and album 102, "Empty", with no tracks.
/// Library 2 holds track 21 on album 201, by artist 401, Far Shore. Nothing is installed.
pub async fn database() -> (tempfile::TempDir, Arc<Database>) {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(&temp.path().join("jewelcase.db")).unwrap());
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        library(tx, 2, "Other", "/other")?;
        let album = |id, library, title: &str, track_count| Album {
            title: Some(title.to_owned()),
            track_count,
            ..Album::new(id, library)
        };
        album(101, 1, "Signal", 2).insert(tx)?;
        album(102, 1, "Empty", 0).insert(tx)?;
        album(201, 2, "Elsewhere", 1).insert(tx)?;
        Artist::new(301, 1, "Aurora Lane").insert(tx)?;
        album_artist(tx, 1, 101, 301)?;
        Artist::new(401, 2, "Far Shore").insert(tx)?;
        album_artist(tx, 2, 201, 401)?;
        for (id, library, album, missing_since) in
            [(11, 1, 101, None), (12, 1, 101, None), (13, 1, 101, Some(5)), (21, 2, 201, None)]
        {
            Track {
                path: format!("Signal/{id}.flac"),
                missing_since,
                track_number: Some(id % 10),
                duration_us: 180_000_000,
                ..Track::new(id, library, album)
            }
            .insert(tx)?;
        }
        track_artist(tx, 1, 11, 0, 301, "Aurora Lane")?;
        track_artist(tx, 2, 21, 0, 401, "Far Shore")
    })
    .await
    .unwrap();
    (temp, db)
}

/// The plugins over `db`, keeping their files in `temp`, with no scanners.
pub fn plugins(temp: &tempfile::TempDir, db: &Arc<Database>) -> Arc<Plugins> {
    Arc::new(Plugins::new(
        db.clone(),
        temp.path().join("plugins"),
        Some(temp.path().join("compiled")),
        HashMap::new(),
    ))
}

/// A manifest for plugin `id`, named after it and built against the contract, that asks for
/// each of `permissions` and requires none of them.
pub fn manifest(id: &str, permissions: &[Permission]) -> Value {
    let requests: Vec<Value> = permissions
        .iter()
        .map(|p| json!({ "permission": p.name(), "required": false, "reason": "r" }))
        .collect();
    json!({
        "id": id, "name": id, "version": "1", "apiVersion": manifest::API_VERSION,
        "permissions": requests
    })
}

/// Installs the plugin `manifest` describes as [`Plugins::install`] leaves it once its file
/// checks out: disabled everywhere, with only the grants it held before whose requests it makes
/// again unchanged.
/// Panics on a manifest the server would refuse, or one installed already.
pub async fn install(db: &Database, manifest: Value) {
    let problems = manifest::validate(&manifest);
    assert!(problems.is_empty(), "{problems:?}");
    let manifest: Manifest = serde_json::from_value(manifest).unwrap();
    assert!(db.write(move |tx| super::add(tx, &manifest, None)).await.unwrap(), "installed");
}

/// Grants `id` exactly `plugin_wide` for itself and `in_library` in library 1, as the admin
/// does.
pub async fn grant(
    plugins: &Plugins,
    id: &str,
    plugin_wide: &[Permission],
    in_library: &[Permission],
) {
    let per_library = vec![(1, in_library.to_vec())];
    plugins.set_grants(id.into(), plugin_wide.to_vec(), per_library).await.unwrap();
}

/// Enables or disables `id` in library 1, as the admin does.
pub async fn enable(plugins: &Plugins, id: &str, enabled: bool) {
    plugins.set_enabled(id.into(), 1, enabled).await.unwrap();
}

/// Enables `id` in library 1 without the permissions it requires, which the admin cannot do.
/// For the tests of a real plugin, which keep it off the network by never granting that.
pub async fn enable_ungranted(db: &Database, id: &'static str) {
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES (?1, 1, 1)",
            [id],
        )
    })
    .await
    .unwrap();
}

/// Connects `user` to `id`, which must be installed, with `settings` at `at`, as saving them
/// does once the plugin accepts them. Only a running plugin can accept them, and the tests'
/// plugins cannot reach the services they check against.
pub async fn connect(db: &Database, id: &'static str, user: i64, settings: Value, at: i64) {
    let connected = db.write(move |tx| personal::connect(tx, id, user, &settings.to_string(), at));
    assert!(connected.await.unwrap(), "{id} is installed");
}
