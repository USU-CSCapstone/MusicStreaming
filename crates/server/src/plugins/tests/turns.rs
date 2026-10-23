//! One run at a time for each plugin in each library: Run now, hook deliveries, and settings
//! checks each hold the plugin's turn there while it runs, and uninstalling waits for them.

use futures_util::FutureExt;
use jewelcase_plugins::Permission::Schedule;
use serde_json::{Map, json};

use super::super::settings::SERVER;
use super::*;

/// Whether `future` is still waiting once it is polled. It is dropped either way.
pub(super) fn waits(future: impl Future) -> bool {
    future.now_or_never().is_none()
}

/// Whether the plugin is installed.
pub(super) async fn installed(db: &Database) -> bool {
    let sql = "SELECT EXISTS (SELECT 1 FROM plugins WHERE id = 'plugin')";
    db.read(move |conn| conn.query_row(sql, [], |row| row.get(0))).await.unwrap()
}

/// Waits until `count` runs hold or wait for the plugin's turn in `library`. The deadline only
/// bounds a hang.
pub(super) async fn queued(plugins: &Plugins, library: i64, count: usize) {
    let queued = async {
        while plugins.turns.waiting("plugin", library) < count {
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(Duration::from_secs(30), queued).await.expect("queued within 30 s");
}

/// Waits for the plugin's turn in library 1, once whatever holds it ends. The deadline only
/// bounds a hang.
async fn ended(plugins: &Plugins) {
    let turn = tokio::time::timeout(Duration::from_secs(30), plugins.turns.take("plugin", 1));
    drop(turn.await.expect("the run ends within 30 s"));
}

/// Run now holds the plugin's turn in the library while it runs, leaving other libraries and
/// other plugins free, and uninstalling waits for it to end.
#[tokio::test]
async fn run_now_holds_its_turn_and_uninstalling_waits_for_it() {
    let (_temp, db, plugins, thread) = scanned().await;
    gate(&touching(&plugins, &db).await);
    let run = tokio::spawn({
        let plugins = plugins.clone();
        async move { plugins.run("plugin".into()).await }
    });
    let loading = loading(plugins.path("plugin")).await;

    assert!(waits(plugins.turns.take("plugin", 1)), "its turn in library 1 is taken");
    assert!(!waits(plugins.turns.take("plugin", 2)), "another library's is not");
    assert!(!waits(plugins.turns.take("other", 1)), "nor another plugin's");
    assert!(waits(plugins.uninstall("plugin".into())));
    // Done once any write the uninstall queued is.
    db.write(|_| Ok(())).await.unwrap();
    assert!(installed(&db).await, "uninstalling waits for the run");

    let_in(loading);
    assert!(run.await.unwrap().unwrap().ok);
    plugins.uninstall("plugin".into()).await.unwrap();
    assert!(!installed(&db).await);
    stop(thread).await;
}

/// Installs the plugin as [`touching`] does, also granted a schedule, so a delivery is due at
/// once in library 1, and makes its file a FIFO ([`gate`]).
async fn scheduled(plugins: &Plugins, db: &Database) {
    let mut asks = manifest("plugin", &[LibraryChange, Schedule]);
    asks["permissions"][1]["everyMinutes"] = json!(60);
    install(db, asks).await;
    grant(plugins, "plugin", &[Schedule], &[LibraryChange]).await;
    enable(plugins, "plugin", true).await;
    std::fs::create_dir_all(plugins.path("plugin").parent().unwrap()).unwrap();
    gate(&plugins.path("plugin"));
}

/// A hook delivery holds the plugin's turn in the library, and one of the hook slots, while it
/// runs, and lets both go once it is recorded.
#[tokio::test]
async fn a_delivery_holds_its_turn_and_a_hook_slot() {
    let (_temp, db, plugins, thread) = scanned().await;
    scheduled(&plugins, &db).await;
    let slots = plugins.hook_slots.available_permits();
    assert_eq!(slots, std::thread::available_parallelism().unwrap().get(), "one for each core");

    plugins.dispatch().await;
    let loading = loading(plugins.path("plugin")).await;
    assert!(waits(plugins.turns.take("plugin", 1)), "its turn is taken");
    assert_eq!(plugins.hook_slots.available_permits(), slots - 1, "and a slot");

    let_in(loading);
    ended(&plugins).await;
    assert_eq!(plugins.hook_slots.available_permits(), slots);
    stop(thread).await;
}

/// A delivery waiting for the plugin's turn holds no hook slot, so it keeps none from the
/// deliveries of other libraries and other plugins.
#[tokio::test]
async fn a_delivery_waiting_for_its_turn_holds_no_slot() {
    let (_temp, db, plugins, thread) = scanned().await;
    scheduled(&plugins, &db).await;
    let slots = plugins.hook_slots.available_permits();
    let turn = plugins.turns.take("plugin", 1).await;

    plugins.dispatch().await;
    queued(&plugins, 1, 2).await;
    assert_eq!(plugins.hook_slots.available_permits(), slots);

    drop(turn);
    let_in(loading(plugins.path("plugin")).await);
    ended(&plugins).await;
    stop(thread).await;
}

/// A delivery whose plugin is disabled while it waits for its turn runs nothing: the plugin is
/// never loaded, and the delivery lets its turn go.
#[tokio::test]
async fn a_delivery_disabled_while_it_waits_runs_nothing() {
    let (_temp, db, plugins, thread) = scanned().await;
    scheduled(&plugins, &db).await;
    let turn = plugins.turns.take("plugin", 1).await;
    plugins.dispatch().await;
    queued(&plugins, 1, 2).await;

    enable(&plugins, "plugin", false).await;
    drop(turn);
    // A delivery that loaded the plugin would hold the turn, waiting at the FIFO.
    ended(&plugins).await;
    stop(thread).await;
}

/// Run now in a library the plugin is disabled in while it waits for its turn runs nothing
/// there: the plugin is never loaded.
#[tokio::test]
async fn run_now_disabled_while_it_waits_runs_nothing() {
    let (_temp, db, plugins, thread) = scanned().await;
    gate(&touching(&plugins, &db).await);
    let turn = plugins.turns.take("plugin", 1).await;
    let run = tokio::spawn({
        let plugins = plugins.clone();
        async move { plugins.run("plugin".into()).await }
    });
    queued(&plugins, 1, 2).await;

    enable(&plugins, "plugin", false).await;
    drop(turn);
    // A run that loaded the plugin would wait at the FIFO.
    let ran = tokio::time::timeout(Duration::from_secs(30), run).await;
    let result = ran.expect("the run ends within 30 s").unwrap().unwrap();
    assert!(!result.ok, "{}", result.summary);
    assert_eq!(result.summary, "plugin is not enabled in any library.");
    stop(thread).await;
}

/// Uninstalling takes the plugin's turn at the server level and in every library, in order, so a
/// check queued behind it finds the plugin gone: it runs nothing, answers that there is no such
/// plugin, and stores nothing.
#[tokio::test]
async fn a_check_queued_behind_an_uninstall_runs_nothing() {
    let (_temp, db, plugins, thread) = scanned().await;
    gate(&touching(&plugins, &db).await);
    let turn = plugins.turns.take("plugin", 1).await;
    let uninstall = tokio::spawn({
        let plugins = plugins.clone();
        async move { plugins.uninstall("plugin".into()).await }
    });
    queued(&plugins, 1, 2).await;
    let check = tokio::spawn({
        let plugins = plugins.clone();
        async move { plugins.save_settings("plugin".into(), SERVER, Map::new()).await }
    });
    queued(&plugins, SERVER, 2).await;

    drop(turn);
    uninstall.await.unwrap().unwrap();
    // A check that loaded the plugin would wait at the FIFO, or fail once it is removed.
    let checked = tokio::time::timeout(Duration::from_secs(30), check).await;
    let checked = checked.expect("the check ends within 30 s").unwrap();
    assert!(matches!(checked, Err(PluginError::NotFound)), "{checked:?}");
    let sql = "SELECT count(*) FROM plugin_settings";
    let stored: i64 = db.read(move |conn| conn.query_row(sql, [], |row| row.get(0))).await.unwrap();
    assert_eq!(stored, 0);
    stop(thread).await;
}

/// A check whose caller stops waiting for its turn leaves the queue, so it never runs.
#[tokio::test]
async fn a_check_left_waiting_for_its_turn_leaves_the_queue() {
    let (_temp, db, plugins, thread) = scanned().await;
    gate(&touching(&plugins, &db).await);
    let turn = plugins.turns.take("plugin", SERVER).await;
    let check = tokio::spawn({
        let plugins = plugins.clone();
        async move { plugins.save_settings("plugin".into(), SERVER, Map::new()).await }
    });
    queued(&plugins, SERVER, 2).await;
    check.abort();
    assert!(check.await.is_err_and(|error| error.is_cancelled()));
    assert_eq!(plugins.turns.waiting("plugin", SERVER), 1, "only this test's turn is left");

    drop(turn);
    assert!(!waits(plugins.turns.take("plugin", SERVER)), "and nothing takes it after");
    stop(thread).await;
}

/// A settings check holds the plugin's turn in the library while it runs, even once its caller
/// stops waiting for it, and what it touched is scanned when it ends.
#[tokio::test]
async fn a_settings_check_holds_its_turn_to_its_end() {
    let (_temp, db, plugins, thread) = scanned().await;
    gate(&touching(&plugins, &db).await);
    let check = tokio::spawn({
        let plugins = plugins.clone();
        async move { plugins.save_settings("plugin".into(), 1, Map::new()).await }
    });
    let loading = loading(plugins.path("plugin")).await;
    assert!(waits(plugins.turns.take("plugin", 1)), "its turn is taken");
    check.abort();
    assert!(check.await.is_err_and(|error| error.is_cancelled()));
    assert!(waits(plugins.turns.take("plugin", 1)), "and kept");

    let_in(loading);
    ended(&plugins).await;
    assert_eq!(watch_scans(&db).await, [["Album"]]);
    stop(thread).await;
}
