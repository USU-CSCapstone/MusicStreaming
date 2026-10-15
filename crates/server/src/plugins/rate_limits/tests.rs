use std::time::Duration;

use rusqlite::params;
use serde_json::json;

use super::super::testing::database;
use super::super::{PluginError, Plugins};
use super::{exemptions, for_run, in_force};
use crate::db::Database;

/// Installs `id`, setting each of `limits` for every plugin, granted that and the network, and
/// enabled in library 1.
async fn setter(db: &Database, id: &'static str, limits: &[(&str, &str)]) {
    let hosts: Vec<_> = limits.iter().map(|(host, _)| *host).collect();
    let limits: serde_json::Map<_, _> = limits
        .iter()
        .map(|(host, limit)| (host.to_string(), json!({ "limit": limit, "per": "host" })))
        .collect();
    let manifest = json!({
        "id": id, "name": id, "version": "1", "apiVersion": "0.3",
        "permissions": [
            { "permission": "network", "required": true, "reason": "r",
              "destinations": hosts, "rateLimits": limits },
            { "permission": "hostRateLimits", "required": false, "reason": "r" }
        ]
    });
    db.write(move |tx| {
        tx.execute(
            "INSERT INTO plugins (id, manifest, installed_at, updated_at) VALUES (?1, ?2, 0, 0)",
            params![id, manifest.to_string()],
        )?;
        tx.execute("INSERT INTO plugin_grants VALUES (?1, 'hostRateLimits')", [id])?;
        tx.execute("INSERT INTO plugin_grants VALUES (?1, 'network')", [id])?;
        tx.execute(
            "INSERT INTO plugin_libraries (plugin_id, library_id, enabled) VALUES (?1, 1, 1)",
            [id],
        )
    })
    .await
    .unwrap();
}

/// Each host limit in force, as the admin sees it.
async fn listed(db: &Database) -> Vec<(String, String, Vec<String>)> {
    let limits = db.read(in_force).await.unwrap();
    limits.into_iter().map(|l| (l.host, l.limit, l.set_by)).collect()
}

fn limit(host: &str, limit: &str, set_by: &[&str]) -> (String, String, Vec<String>) {
    (host.into(), limit.into(), set_by.iter().map(|id| id.to_string()).collect())
}

#[tokio::test]
async fn the_strictest_limit_set_for_a_host_is_in_force() {
    let (_temp, db) = database().await;
    setter(&db, "lenient", &[("musicbrainz.org", "2/s"), ("api.example", "1/s")]).await;
    setter(&db, "strict", &[("MusicBrainz.org", "60/min")]).await;
    setter(&db, "also-strict", &[("musicbrainz.org", "1/s")]).await;
    assert_eq!(
        listed(&db).await,
        [
            limit("api.example", "1/s", &["lenient"]),
            limit("musicbrainz.org", "1/s", &["also-strict", "strict"]),
        ]
    );
    let run = db.read(|conn| for_run(conn, "anyone")).await.unwrap();
    let second = Duration::from_secs(1);
    assert_eq!(run, [("api.example".into(), second), ("musicbrainz.org".into(), second)]);
}

#[tokio::test]
async fn a_limit_is_in_force_only_while_its_plugin_is_installed_granted_both_and_enabled() {
    let (_temp, db) = database().await;
    setter(&db, "strict", &[("musicbrainz.org", "1/s")]).await;
    setter(&db, "stricter", &[("musicbrainz.org", "1/min")]).await;
    let stricter = || limit("musicbrainz.org", "1/min", &["stricter"]);
    let strict = || limit("musicbrainz.org", "1/s", &["strict"]);
    assert_eq!(listed(&db).await, [stricter()]);
    for (undo, redo) in [
        (
            "UPDATE plugin_libraries SET enabled = 0 WHERE plugin_id = 'stricter'",
            "UPDATE plugin_libraries SET enabled = 1 WHERE plugin_id = 'stricter'",
        ),
        (
            "DELETE FROM plugin_grants WHERE plugin_id = 'stricter' \
             AND permission = 'hostRateLimits'",
            "INSERT INTO plugin_grants VALUES ('stricter', 'hostRateLimits')",
        ),
        // The limit is on destinations it is allowed to reach (`requirements/plugins.md` §4.1).
        (
            "DELETE FROM plugin_grants WHERE plugin_id = 'stricter' AND permission = 'network'",
            "INSERT INTO plugin_grants VALUES ('stricter', 'network')",
        ),
    ] {
        db.write(move |tx| tx.execute_batch(undo)).await.unwrap();
        assert_eq!(listed(&db).await, [strict()], "{undo}");
        db.write(move |tx| tx.execute_batch(redo)).await.unwrap();
        assert_eq!(listed(&db).await, [stricter()], "{redo}");
    }
    // Uninstalled, its grants are kept for a reinstall, but its limit is not in force.
    db.write(|tx| tx.execute_batch("DELETE FROM plugins WHERE id = 'stricter'")).await.unwrap();
    assert_eq!(listed(&db).await, [strict()]);
}

#[tokio::test]
async fn a_manifest_that_cannot_be_read_is_left_out_rather_than_stopping_every_run() {
    let (_temp, db) = database().await;
    setter(&db, "strict", &[("musicbrainz.org", "1/s")]).await;
    setter(&db, "broken", &[("api.example", "1/s")]).await;
    // Written by a version of the server that knew a contract this one does not.
    let broken = "UPDATE plugins SET manifest = json_set(manifest, '$.apiVersion', '9.9') \
                  WHERE id = 'broken'";
    db.write(move |tx| tx.execute(broken, [])).await.unwrap();
    assert_eq!(listed(&db).await, [limit("musicbrainz.org", "1/s", &["strict"])]);
}

#[tokio::test]
async fn a_run_keeps_to_every_limit_but_those_its_plugin_is_exempt_from() {
    let (_temp, db) = database().await;
    setter(&db, "strict", &[("musicbrainz.org", "1/s"), ("api.example", "1/s")]).await;
    let p = r#"{"id":"p","name":"P","version":"1","apiVersion":"0.3","permissions":[]}"#;
    db.write(move |tx| tx.execute("UPDATE plugins SET manifest = ?1 WHERE id = 'p'", [p]))
        .await
        .unwrap();
    let plugins = Plugins::new(db.clone(), "/nowhere".into(), Default::default());
    for host in ["musicbrainz.org", "unlimited.example", "musicbrainz.org"] {
        plugins.add_exemption("p".into(), host.into()).await.unwrap();
    }
    let exempt = db.read(|conn| exemptions(conn, "p")).await.unwrap();
    assert_eq!(exempt, ["musicbrainz.org", "unlimited.example"], "each once");
    let run = db.read(|conn| for_run(conn, "p")).await.unwrap();
    assert_eq!(run, [("api.example".into(), Duration::from_secs(1))]);
    // Exempting it from its own limit leaves it out of its own runs too.
    plugins.add_exemption("strict".into(), "api.example".into()).await.unwrap();
    let run = db.read(|conn| for_run(conn, "strict")).await.unwrap();
    assert_eq!(run, [("musicbrainz.org".into(), Duration::from_secs(1))]);

    for _ in 0..2 {
        plugins.remove_exemption("p".into(), "musicbrainz.org".into()).await.unwrap();
    }
    let exempt = db.read(|conn| exemptions(conn, "p")).await.unwrap();
    assert_eq!(exempt, ["unlimited.example"], "removed, and removing again changes nothing");
    let missing = plugins.add_exemption("nothing".into(), "api.example".into()).await;
    assert!(matches!(missing, Err(PluginError::NotFound)));
    let missing = plugins.remove_exemption("nothing".into(), "api.example".into()).await;
    assert!(matches!(missing, Err(PluginError::NotFound)));
}
