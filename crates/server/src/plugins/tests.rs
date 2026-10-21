use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use jewelcase_plugins::Permission::LibraryRead;
use jewelcase_scanner::{ScanOptions, Scanner};

use super::testing::{database, enable, grant, install, manifest, plugins};
use super::{PluginError, Plugins};
use crate::db::{Database, SqliteStore, libraries};

/// Queuing a scan of what a plugin saved, from inside the async runtime as a request does,
/// neither panics nor stops the scanner from taking more work.
#[tokio::test]
async fn saved_files_are_scanned_without_blocking_the_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let music = temp.path().join("music");
    std::fs::create_dir_all(music.join("Album")).unwrap();
    let db = Arc::new(Database::open(&temp.path().join("jewelcase.db")).unwrap());
    let root = music.clone();
    db.write(move |tx| libraries::create(tx, Some(1), "Music", &[&root], &[])).await.unwrap();
    let config = db.read(libraries::all).await.unwrap()[0].config();
    // Started outside the runtime, as `run` starts every scanner.
    let store_db = db.clone();
    let scanner = tokio::task::spawn_blocking(move || {
        let store = Arc::new(SqliteStore::new(store_db).unwrap());
        Scanner::start(store, config, ScanOptions::default())
    })
    .await
    .unwrap();
    let plugins = Plugins::new(
        db,
        temp.path().join("plugins"),
        Some(temp.path().join("compiled")),
        HashMap::from([(1, scanner.clone())]),
    );

    let saved = [music.join("Album/01.lrc"), music.join("Album/02.lrc")];
    assert!(plugins.scan(1, &saved).await);
    assert!(plugins.scan(1, &[music.join("Album/03.lrc")]).await, "the scanner still takes work");
    assert!(!plugins.scan(2, &saved).await, "a library with no scanner");
    assert!(
        !plugins.scan(1, &[Path::new("/elsewhere/x.lrc").to_path_buf()]).await,
        "outside its roots"
    );
    tokio::task::spawn_blocking(move || scanner.shutdown()).await.unwrap();
}

/// Run now answers a run that failed as failed, with why, and records it so for the admin: here
/// a plugin installed without its file, which cannot be loaded.
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
    let sql = "SELECT last_run_ok, last_run_summary FROM plugin_libraries";
    let recorded: (bool, String) = db
        .read(move |conn| conn.query_row(sql, [], |row| Ok((row.get(0)?, row.get(1)?))))
        .await
        .unwrap();
    assert_eq!(recorded, (false, summary));
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
