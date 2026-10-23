//! What a run touched is scanned, in one scan however many folders it touched, whoever started
//! the run and whether or not they wait for it.

use serde_json::Map;

use super::super::settings::SERVER;
use super::*;

/// One scan covers every folder a run touched, however many.
#[tokio::test]
async fn a_run_queues_one_scan_of_every_folder_it_touched() {
    let (temp, db, plugins, thread) = scanned().await;
    let music = temp.path().join("music");
    let touched =
        [music.join("Album/01.lrc"), music.join("Other/02.lrc"), music.join("Album/CD2/03.lrc")];
    assert!(plugins.scan(1, &touched).await);
    assert_eq!(watch_scans(&db).await, [["Album", "Other"]]);
    stop(thread).await;
}

/// Run now goes on to its end when its caller stops waiting, as a request does when its client
/// gives up: what the plugin touched is scanned, and the run is recorded for admins.
#[tokio::test]
async fn a_run_left_waiting_is_still_scanned_and_recorded() {
    let (_temp, db, plugins, thread) = scanned().await;
    gate(&touching(&plugins, &db).await);
    let run = tokio::spawn({
        let plugins = plugins.clone();
        async move { plugins.run("plugin".into()).await }
    });
    let loading = loading(plugins.path("plugin")).await;
    run.abort();
    assert!(run.await.is_err_and(|error| error.is_cancelled()));

    let_in(loading);
    // The run lets its turn go once it is recorded. The deadline only bounds a hang.
    let ended = tokio::time::timeout(Duration::from_secs(30), plugins.turns.take("plugin", 1));
    drop(ended.await.expect("the run ends within 30 s"));
    let (ok, report) = last_run(&db).await;
    assert!(ok && report.starts_with("done (about "), "{report}");
    assert_eq!(watch_scans(&db).await, [["Album"]]);
    stop(thread).await;
}

/// A settings check runs nothing in a library the plugin is disabled in, as a disabled plugin
/// runs nothing (`requirements/plugins.md` §5); server-wide, and where it is enabled, it is
/// checked. This plugin has no file, so a check of it fails.
#[tokio::test]
async fn a_check_runs_only_where_the_plugin_is_enabled() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    install(&db, manifest("plugin", &[LibraryRead])).await;
    grant(&plugins, "plugin", &[], &[LibraryRead]).await;
    let save = |level| plugins.save_settings("plugin".into(), level, Map::new());
    let unloadable = |saved: Result<(), PluginError>| {
        matches!(saved, Err(PluginError::SettingsInvalid(why))
            if why.starts_with("the plugin could not be loaded"))
    };

    save(1).await.unwrap();
    assert!(unloadable(save(SERVER).await));
    enable(&plugins, "plugin", true).await;
    assert!(unloadable(save(1).await));
    enable(&plugins, "plugin", false).await;
    save(1).await.unwrap();
}

/// What a settings check touched in the library is scanned, as after any run.
#[tokio::test]
async fn a_check_is_scanned_as_any_run() {
    let (_temp, db, plugins, thread) = scanned().await;
    std::fs::write(touching(&plugins, &db).await, TOUCHER).unwrap();
    plugins.save_settings("plugin".into(), 1, Map::new()).await.unwrap();
    assert_eq!(watch_scans(&db).await, [["Album"]]);
    stop(thread).await;
}
