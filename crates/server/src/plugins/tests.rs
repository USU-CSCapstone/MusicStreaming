use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use jewelcase_scanner::scan::ScanOptions;
use jewelcase_scanner::{Governor, Scanner};

use super::Plugins;
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
        Scanner::start(store, config, Arc::new(Governor::new()), ScanOptions::default())
    })
    .await
    .unwrap();
    let plugins =
        Plugins::new(db, temp.path().join("plugins"), HashMap::from([(1, scanner.clone())]));

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
