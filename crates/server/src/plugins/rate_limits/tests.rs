use std::time::Duration;

use jewelcase_plugins::Permission::{HostRateLimits, Network};
use serde_json::json;

use super::super::testing::{database, enable, grant, install, manifest, plugins};
use super::super::{PluginError, Plugins};
use super::{exemptions, for_run, in_force};
use crate::db::Database;

/// Installs `id`, setting each of `limits` for every plugin, granted that and the network, and
/// enabled in library 1. It requires neither, so either can be revoked while it stays enabled.
async fn setter(plugins: &Plugins, db: &Database, id: &str, limits: &[(&str, &str)]) {
    let hosts: Vec<_> = limits.iter().map(|(host, _)| *host).collect();
    let limits: serde_json::Map<_, _> = limits
        .iter()
        .map(|(host, limit)| (host.to_string(), json!({ "limit": limit, "per": "host" })))
        .collect();
    let mut manifest = manifest(id, &[Network, HostRateLimits]);
    manifest["permissions"][0]["destinations"] = json!(hosts);
    manifest["permissions"][0]["rateLimits"] = json!(limits);
    install(db, manifest).await;
    grant(plugins, id, &[Network, HostRateLimits], &[]).await;
    enable(plugins, id, true).await;
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
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    setter(&plugins, &db, "lenient", &[("musicbrainz.org", "2/s"), ("api.example", "1/s")]).await;
    setter(&plugins, &db, "strict", &[("MusicBrainz.org", "60/min")]).await;
    setter(&plugins, &db, "also-strict", &[("musicbrainz.org", "1/s")]).await;
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
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    setter(&plugins, &db, "strict", &[("musicbrainz.org", "1/s")]).await;
    setter(&plugins, &db, "stricter", &[("musicbrainz.org", "1/min")]).await;
    let stricter = || limit("musicbrainz.org", "1/min", &["stricter"]);
    let strict = || limit("musicbrainz.org", "1/s", &["strict"]);
    assert_eq!(listed(&db).await, [stricter()]);

    enable(&plugins, "stricter", false).await;
    assert_eq!(listed(&db).await, [strict()], "disabled");
    enable(&plugins, "stricter", true).await;
    assert_eq!(listed(&db).await, [stricter()], "enabled again");
    // The limit is on destinations it is allowed to reach (`requirements/plugins.md` §4.1).
    for (kept, revoked) in [(Network, HostRateLimits), (HostRateLimits, Network)] {
        grant(&plugins, "stricter", &[kept], &[]).await;
        assert_eq!(listed(&db).await, [strict()], "{revoked:?} revoked");
        grant(&plugins, "stricter", &[Network, HostRateLimits], &[]).await;
        assert_eq!(listed(&db).await, [stricter()], "{revoked:?} granted again");
    }
    // Uninstalled, its grants are kept for a reinstall, but its limit is not in force.
    plugins.uninstall("stricter".into()).await.unwrap();
    assert_eq!(listed(&db).await, [strict()]);
}

#[tokio::test]
async fn a_manifest_that_cannot_be_read_is_left_out_rather_than_stopping_every_run() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    setter(&plugins, &db, "strict", &[("musicbrainz.org", "1/s")]).await;
    setter(&plugins, &db, "broken", &[("api.example", "1/s")]).await;
    // Written by a version of the server that knew a contract this one does not.
    let broken = "UPDATE plugins SET manifest = json_set(manifest, '$.apiVersion', '9.9') \
                  WHERE id = 'broken'";
    db.write(move |tx| tx.execute(broken, [])).await.unwrap();
    assert_eq!(listed(&db).await, [limit("musicbrainz.org", "1/s", &["strict"])]);
}

#[tokio::test]
async fn a_run_keeps_to_every_limit_but_those_its_plugin_is_exempt_from() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    setter(&plugins, &db, "strict", &[("musicbrainz.org", "1/s"), ("api.example", "1/s")]).await;
    install(&db, manifest("plugin", &[])).await;
    for host in ["musicbrainz.org", "unlimited.example", "musicbrainz.org"] {
        plugins.add_exemption("plugin".into(), host.into()).await.unwrap();
    }
    let exempt = db.read(|conn| exemptions(conn, "plugin")).await.unwrap();
    assert_eq!(exempt, ["musicbrainz.org", "unlimited.example"], "each once");
    let run = db.read(|conn| for_run(conn, "plugin")).await.unwrap();
    assert_eq!(run, [("api.example".into(), Duration::from_secs(1))]);
    // Exempting it from its own limit leaves it out of its own runs too.
    plugins.add_exemption("strict".into(), "api.example".into()).await.unwrap();
    let run = db.read(|conn| for_run(conn, "strict")).await.unwrap();
    assert_eq!(run, [("musicbrainz.org".into(), Duration::from_secs(1))]);

    for _ in 0..2 {
        plugins.remove_exemption("plugin".into(), "musicbrainz.org".into()).await.unwrap();
    }
    let exempt = db.read(|conn| exemptions(conn, "plugin")).await.unwrap();
    assert_eq!(exempt, ["unlimited.example"], "removed, and removing again changes nothing");
    let missing = plugins.add_exemption("nothing".into(), "api.example".into()).await;
    assert!(matches!(missing, Err(PluginError::NotFound)));
    let missing = plugins.remove_exemption("nothing".into(), "api.example".into()).await;
    assert!(matches!(missing, Err(PluginError::NotFound)));
}

#[tokio::test]
async fn an_exemption_outlives_a_reinstall_only_with_its_network_request_unchanged() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    // Another plugin's network grant keeps nothing of this one's.
    setter(&plugins, &db, "other", &[("other.example", "1/s")]).await;
    setter(&plugins, &db, "plugin", &[("musicbrainz.org", "1/s")]).await;
    let asked: serde_json::Value = db
        .read(|conn| {
            conn.query_row("SELECT manifest FROM plugins WHERE id = 'plugin'", [], |row| row.get(0))
        })
        .await
        .map(|json: String| serde_json::from_str(&json).unwrap())
        .unwrap();
    let mut wider = asked.clone();
    wider["permissions"][0]["destinations"] = json!(["musicbrainz.org", "other.example"]);
    let mut faster = asked.clone();
    faster["permissions"][0]["rateLimits"]["musicbrainz.org"]["limit"] = json!("2/s");
    for (manifest, kept) in [(asked, true), (wider, false), (faster, false)] {
        grant(&plugins, "plugin", &[Network, HostRateLimits], &[]).await;
        plugins.add_exemption("plugin".into(), "api.example".into()).await.unwrap();
        plugins.uninstall("plugin".into()).await.unwrap();
        install(&db, manifest.clone()).await;
        let exempt = db.read(|conn| exemptions(conn, "plugin")).await.unwrap();
        assert_eq!(exempt.len(), usize::from(kept), "{manifest}");
    }
}
