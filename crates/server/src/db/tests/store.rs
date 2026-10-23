//! The SQLite store against the scanner's pipeline suite, plus the
//! derived-data rules the schema assigns to the scanner and the scanning benchmarks.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use jewelcase_core::Format;
use jewelcase_scanner::testing::{self, ItemKey};
use jewelcase_scanner::*;
use rusqlite::OptionalExtension;

use super::super::store::SqliteStore;
use super::super::{Database, libraries};

/// The store [`library_store`] makes for the library with its one root at `root`.
fn make(root: &Path) -> SqliteStore {
    library_store(&testing::library(root))
}

/// The suite's [`testing::Factory`]: a store holding `lib`, with id 1 and every one of its roots,
/// whose database lives beside its first root.
fn library_store(lib: &LibraryConfig) -> SqliteStore {
    let dir = lib.roots[0].parent().unwrap().join("state");
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Database::open(&dir.join("jewelcase.db")).unwrap());
    let roots = lib.roots.clone();
    db.write_blocking(move |tx| {
        let exists: Option<i64> =
            tx.query_row("SELECT id FROM libraries WHERE id = 1", [], |r| r.get(0)).optional()?;
        if exists.is_none() {
            let roots: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
            libraries::create(tx, Some(1), "Test", &roots, &[])?;
        }
        Ok(())
    })
    .unwrap();
    SqliteStore::new(db).unwrap()
}

mod suite {
    jewelcase_scanner::store_suite!(super::library_store);
}

mod album_artists;
mod albums;
mod artist_sort;
mod artists;
mod benchmark;
mod derived;
mod fails_loudly;
mod failures;
mod groups;
mod growth;
mod inspect;
mod isolation;
mod labels;
mod lyrics;
mod marks;
mod primary;
mod purge;
mod reanalysis;
mod recompute;
mod results;
mod scans;
mod sidecars;
mod statistics;
mod totals;
mod track_sort;
mod track_totals;
mod walked;

/// The count `sql` selects from `store`.
fn count(store: &SqliteStore, sql: &'static str) -> i64 {
    store.db().read_blocking(move |conn| conn.query_row(sql, [], |r| r.get(0))).unwrap()
}

/// The one value `sql` selects from `store`, if it selects a row.
fn scalar<T>(store: &SqliteStore, sql: &'static str) -> Option<T>
where
    T: rusqlite::types::FromSql + Send + 'static,
{
    store.db().read_blocking(move |conn| conn.query_row(sql, [], |r| r.get(0)).optional()).unwrap()
}

/// Writes a small WAV at `rel` under `root`, tagged with `items`.
fn tagged(root: &Path, rel: &str, items: &[(ItemKey, &str)]) {
    let p = root.join(rel);
    testing::write_small_wav(&p);
    testing::tag_wav_with(&p, items);
}

/// Retags the file at `rel` under `root` with `items`, dated later so the next scan reads it.
fn retag(root: &Path, rel: &str, items: &[(ItemKey, &str)]) {
    let path = root.join(rel);
    testing::tag_wav_with(&path, items);
    let later = SystemTime::now() + Duration::from_secs(5);
    File::open(&path).unwrap().set_modified(later).unwrap();
}

/// Synthetic record `i` under `root`, as the scanner makes for a new file: ten tracks to an
/// album, ten albums to an artist, and three genres.
fn record(root: &Path, i: usize) -> TrackRecord {
    let artist = format!("Artist {}", i / 100);
    let album = format!("Album {}", i / 10);
    let path = root.join(format!("{artist}/{album}/{:02} - Track {i}.flac", i % 10 + 1));
    TrackRecord {
        id: None,
        root: root.to_path_buf(),
        path,
        size: 30_000_000 + i as u64,
        mtime_ms: 1_700_000_000_000 + i as u64,
        tags: TagSet {
            title: Some(format!("Track {i}")),
            artists: vec![artist.clone()],
            album: Some(album),
            album_artists: vec![artist],
            track_number: Some((i % 10 + 1) as u32),
            track_total: Some(10),
            genres: vec![format!("Genre {}", i % 3)],
            release_date: PartialDate::parse("1999"),
            ..Default::default()
        },
        properties: AudioProperties {
            format: Format::Flac,
            duration_ms: 240_000,
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate_kbps: Some(900),
        },
        artwork: None,
        artist_image: None,
        artist_biography: None,
        lyrics_sidecar: None,
        sidecar_stamp: 0,
        sort: SortKeys {
            title: format!("track {i}"),
            artist: format!("artist {}", i / 100),
            album: format!("album {}", i / 10),
        },
    }
}
