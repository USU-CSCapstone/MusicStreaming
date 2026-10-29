//! Running a plugin: what Run now answers and records, a file that cannot compile, and scanning
//! what a run touched.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use jewelcase_plugins::Permission::{LibraryChange, LibraryRead, TagTypes};
use jewelcase_scanner::{Scanner, ScannerThread};
use tempfile::TempDir;

use super::testing::{database, enable, grant, install, manifest, plugins};
use super::{PluginError, Plugins};
use crate::api::testing::eventually;
use crate::db::{Database, SqliteStore, libraries};

mod scans;
mod turns;

/// A plugin written by hand that touches `Album/gone.lrc` in root 1 on any event, and answers
/// that it is done.
pub(super) const TOUCHER: &str = include_str!("tests/toucher.wat");

/// A plugin written by hand that replaces `Album/01.wav` in root 1 on any event with a WAV whose
/// `ENERGY` field is `High`, and answers that it is done.
const ENERGIZER: &str = include_str!("tests/energizer.wat");

/// A library of its own, library 1, whose one root is `music` in the temporary folder, with a
/// scanner, and the plugins over it. The root's ID is 1, so [`TOUCHER`] can name it. The scanner
/// runs until its thread is dropped, which [`stop`] does.
async fn scanned() -> (TempDir, Arc<Database>, Arc<Plugins>, ScannerThread) {
    let (temp, db) = crate::db::testing::open();
    let music = temp.path().join("music");
    std::fs::create_dir_all(music.join("Album")).unwrap();
    db.write(move |tx| {
        libraries::create(tx, Some(1), "Music", &[], &[])?;
        tx.execute(
            "INSERT INTO library_roots (id, library_id, path, created_at) VALUES (1, 1, ?1, 0)",
            [music.to_string_lossy()],
        )
    })
    .await
    .unwrap();
    let config = db.read(libraries::all).await.unwrap()[0].config();
    // Started outside the runtime, as `Scanning::start` starts every scanner.
    let store_db = db.clone();
    let (scanner, thread) = tokio::task::spawn_blocking(move || {
        let store = Arc::new(SqliteStore::new(store_db).unwrap());
        Scanner::start(store, config).unwrap()
    })
    .await
    .unwrap();
    let plugins = Arc::new(Plugins::new(
        db.clone(),
        temp.path().join("plugins"),
        HashMap::from([(1, scanner)]),
    ));
    (temp, db, plugins, thread)
}

/// Stops the scanner `thread` started, off the async threads, as dropping it waits for it.
async fn stop(thread: ScannerThread) {
    tokio::task::spawn_blocking(move || drop(thread)).await.unwrap();
}

/// Installs the plugin, granted to change files in library 1 and enabled there, and answers
/// where its file goes: [`TOUCHER`], or a FIFO from [`gate`].
async fn touching(plugins: &Plugins, db: &Database) -> PathBuf {
    install(db, manifest("plugin", &[LibraryChange])).await;
    grant(plugins, "plugin", &[], &[LibraryChange]).await;
    enable(plugins, "plugin", true).await;
    let file = plugins.path("plugin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    file
}

/// Makes `path` a FIFO, so the first run of the plugin installed there waits, as it loads it,
/// until [`let_in`] hands it its code.
fn gate(path: &Path) {
    assert!(std::process::Command::new("mkfifo").arg(path).status().unwrap().success());
}

/// Waits until a run is loading the plugin through the FIFO at `path`, and answers the end that
/// hands it its code. The deadline only bounds a hang.
async fn loading(path: PathBuf) -> File {
    let opened = tokio::task::spawn_blocking(move || File::options().write(true).open(path));
    let opened = tokio::time::timeout(Duration::from_secs(30), opened).await;
    opened.expect("a run loads the plugin within 30 s").unwrap().unwrap()
}

/// Hands the run that is `loading` the plugin [`TOUCHER`].
fn let_in(mut loading: File) {
    loading.write_all(TOUCHER.as_bytes()).unwrap();
}

/// The last run recorded in library 1: whether it went well, and its report.
async fn last_run(db: &Database) -> (bool, String) {
    let sql = "SELECT last_run_ok, last_run_summary FROM plugin_libraries WHERE library_id = 1";
    db.read(move |conn| conn.query_row(sql, [], |row| Ok((row.get(0)?, row.get(1)?))))
        .await
        .unwrap()
}

/// The watch scans of library 1 queued so far, by the paths of their scopes.
async fn watch_scans(db: &Database) -> Vec<Vec<String>> {
    let sql = "SELECT (SELECT json_group_array(json_extract(s.value, '$.path')) \
                       FROM json_each(scans.scopes) s) \
               FROM scans WHERE library_id = 1 AND trigger = 'watch' ORDER BY id";
    let scans: Vec<String> = db
        .read(move |conn| conn.prepare(sql)?.query_map([], |row| row.get(0))?.collect())
        .await
        .unwrap();
    scans.iter().map(|paths| serde_json::from_str(paths).unwrap()).collect()
}

/// Queuing a scan of what a plugin saved, from inside the async runtime as a request does,
/// neither panics nor stops the scanner from taking more work.
#[tokio::test]
async fn saved_files_are_scanned_without_blocking_the_runtime() {
    let (temp, _db, plugins, thread) = scanned().await;
    let music = temp.path().join("music");

    let saved = [music.join("Album/01.lrc"), music.join("Album/02.lrc")];
    assert!(plugins.scan(1, &saved).await);
    assert!(plugins.scan(1, &[music.join("Album/03.lrc")]).await, "the scanner still takes work");
    assert!(!plugins.scan(2, &saved).await, "a library with no scanner");
    assert!(
        !plugins.scan(1, &[Path::new("/elsewhere/x.lrc").to_path_buf()]).await,
        "outside its roots"
    );
    stop(thread).await;
}

/// Each tag a track of library 1 has of `type`, by name.
async fn tagged(db: &Database, r#type: &'static str) -> Vec<String> {
    let sql = "SELECT tag_name FROM track_tags tagged JOIN tags tag ON tag.id = tagged.tag_id \
               WHERE tag.library_id = 1 AND tag.type = ?1 ORDER BY tag_name";
    let names =
        db.read(move |conn| conn.prepare(sql)?.query_map([r#type], |r| r.get(0))?.collect());
    names.await.unwrap()
}

/// How many scans of library 1 have been queued as `reconfigure`.
async fn reconfigure_scans(db: &Database) -> i64 {
    let sql = "SELECT count(*) FROM scans WHERE library_id = 1 AND trigger = 'reconfigure'";
    db.read(move |conn| conn.query_row(sql, [], |r| r.get(0))).await.unwrap()
}

/// A manifest for `plugin` asking for `permissions`, among them `tagTypes`, which adds the type
/// `energy`, named "Energy" and read from `ENERGY`.
fn adding_energy(permissions: &[jewelcase_plugins::Permission]) -> serde_json::Value {
    let mut manifest = manifest("plugin", permissions);
    let types = serde_json::json!([{ "key": "energy", "name": "Energy", "field": "ENERGY" }]);
    for request in manifest["permissions"].as_array_mut().unwrap() {
        if request["permission"] == "tagTypes" {
            request["types"] = types.clone();
        }
    }
    manifest
}

/// Approving a plugin's tag types scans every library for them, so a file already indexed shows
/// its values without a rescan asked for; approving them again scans nothing.
#[tokio::test]
async fn approved_tag_types_are_read_from_files_already_indexed() {
    let (temp, db, plugins, thread) = scanned().await;
    let wav = temp.path().join("music/Album/01.wav");
    jewelcase_scanner::testing::write_small_wav(&wav);
    jewelcase_scanner::testing::tag_wav_with_field(&wav, "ENERGY", "High");
    plugins.scan_every_library().await;
    let indexed = "SELECT count(*) FROM tracks t JOIN scans s ON s.library_id = t.library_id \
                   WHERE t.library_id = 1 AND s.state = 'completed'";
    eventually("the library is indexed", async || {
        let count: i64 =
            db.read(move |conn| conn.query_row(indexed, [], |r| r.get(0))).await.unwrap();
        (count == 1).then_some(())
    })
    .await;
    assert_eq!(tagged(&db, "plugin.energy").await, Vec::<String>::new());

    install(&db, adding_energy(&[TagTypes])).await;
    grant(&plugins, "plugin", &[TagTypes], &[]).await;
    assert_eq!(reconfigure_scans(&db).await, 2, "the approval queued one");
    let names = eventually("the track is tagged", async || {
        Some(tagged(&db, "plugin.energy").await).filter(|names| !names.is_empty())
    })
    .await;
    assert_eq!(names, ["High"]);

    grant(&plugins, "plugin", &[TagTypes], &[]).await;
    assert_eq!(reconfigure_scans(&db).await, 2, "approved as it was, it reads nothing new");
    stop(thread).await;
}

/// A field a plugin writes into a file, as a tagger does, is read as tags of the type it added,
/// by the scan of what its run touched (`requirements/tags.md` §2).
#[tokio::test]
async fn a_field_a_plugin_writes_is_read_as_the_type_it_added() {
    let (temp, db, plugins, thread) = scanned().await;
    jewelcase_scanner::testing::write_small_wav(&temp.path().join("music/Album/01.wav"));
    install(&db, adding_energy(&[LibraryChange, TagTypes])).await;
    grant(&plugins, "plugin", &[TagTypes], &[LibraryChange]).await;
    enable(&plugins, "plugin", true).await;
    let file = plugins.path("plugin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, ENERGIZER).unwrap();

    let run = plugins.run("plugin".into()).await.unwrap();
    assert_eq!((run.ok, run.summary.as_str(), run.saved), (true, "done", 1));
    assert_eq!(watch_scans(&db).await, [["Album"]], "what it wrote is scanned");
    let names = eventually("the track is tagged", async || {
        Some(tagged(&db, "plugin.energy").await).filter(|names| !names.is_empty())
    })
    .await;
    assert_eq!(names, ["High"]);
    stop(thread).await;
}

/// Run now answers a run that failed as failed, with why, and records it so for the admin, with
/// the compute it spent: here a plugin installed without its file, which cannot be loaded, so
/// it spent none.
#[tokio::test]
async fn a_failed_run_is_answered_and_recorded_as_failed() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    install(&db, manifest("plugin", &[LibraryRead])).await;
    grant(&plugins, "plugin", &[], &[LibraryRead]).await;
    enable(&plugins, "plugin", true).await;

    let result = plugins.run("plugin".into()).await.unwrap();
    assert!(!result.ok);
    let summary = result.summary;
    assert!(summary.starts_with("the plugin could not be loaded: "), "{summary}");
    assert_eq!(last_run(&db).await, (false, format!("{summary} (about 0 ms of compute)")));
}

/// A run that fails on the server's side, here on settings it cannot read, answers the error
/// and is recorded as failed for the admin, in fixed words that leave out the database's message.
#[tokio::test]
async fn a_run_that_fails_on_the_servers_side_is_recorded_as_failed() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    install(&db, manifest("plugin", &[LibraryRead])).await;
    grant(&plugins, "plugin", &[], &[LibraryRead]).await;
    enable(&plugins, "plugin", true).await;
    db.write(|tx| {
        tx.execute_batch(
            "PRAGMA ignore_check_constraints = ON; \
             INSERT INTO plugin_settings (plugin_id, settings) VALUES ('plugin', 'not JSON'); \
             PRAGMA ignore_check_constraints = OFF;",
        )
    })
    .await
    .unwrap();

    assert!(matches!(plugins.run("plugin".into()).await, Err(PluginError::Db(_))));
    let failed = (false, "the server could not complete the run".to_owned());
    assert_eq!(last_run(&db).await, failed);
}

/// A run whose report cannot be recorded, here for a disk the database finds full, still
/// answers how it went.
#[tokio::test]
async fn a_run_whose_report_cannot_be_recorded_still_answers() {
    let (temp, db) = database().await;
    let plugins = plugins(&temp, &db);
    install(&db, manifest("plugin", &[LibraryRead])).await;
    grant(&plugins, "plugin", &[], &[LibraryRead]).await;
    enable(&plugins, "plugin", true).await;
    let refuse = "CREATE TRIGGER full BEFORE UPDATE OF last_run_at ON plugin_libraries \
                  BEGIN SELECT RAISE(ABORT, 'the database or disk is full'); END";
    db.write(move |tx| tx.execute_batch(refuse)).await.unwrap();

    let result = plugins.run("plugin".into()).await.unwrap();
    assert!(!result.ok);
    let summary = result.summary;
    assert!(summary.starts_with("the plugin could not be loaded: "), "{summary}");
}

/// A component that carries a valid manifest, but whose one core module declares a function of
/// a type it does not have: framed well enough for the manifest to be read, then refused when
/// compiled.
fn uncompilable() -> Vec<u8> {
    let module = b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x03\x02\x01\x05\x0a\x04\x01\x02\0\x0b";
    let (name, manifest) =
        (jewelcase_plugins::manifest::SECTION, manifest("broken", &[]).to_string());
    let section = [&[name.len() as u8], name.as_bytes(), manifest.as_bytes()].concat();
    assert!(section.len() < 128, "its size fits in one byte");
    let header = b"\0asm\x0d\0\x01\0";
    [&header[..], &[1, module.len() as u8], module, &[0, section.len() as u8], &section].concat()
}

/// A file that cannot run here is refused with every cause wasmtime gives, not only its root
/// cause.
#[tokio::test]
async fn a_plugin_that_cannot_compile_is_refused_with_why() {
    let (temp, db) = database().await;
    match plugins(&temp, &db).install(uncompilable(), None).await {
        Err(PluginError::Invalid(why)) => {
            let compile = "This plugin cannot run here: failed to parse WebAssembly module: ";
            assert!(why.starts_with(compile) && why.contains("out of bounds"), "{why}");
        }
        other => panic!("{other:?}"),
    }
}
