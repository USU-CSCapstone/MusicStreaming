//! The hourly sweep's purge of tracks missing for 30 days and still missing after a later scan
//! (`requirements/scanning.md` §8): what they leave of the catalog follows in the same
//! transaction, and the feed says so.

use std::fs;
use std::path::{Path, PathBuf};

use jewelcase_scanner::testing::{self, InspectStore, ItemKey};
use jewelcase_scanner::{Depth, ScanState, Scope, Trigger};
use rusqlite::Connection;
use rusqlite::types::Value;

use super::recompute::{assert_fed, check, snapshot};
use super::{count, make, scalar, tagged};
use crate::db::now_ms;
use crate::db::retention::WINDOW_MS;
use crate::db::store::SqliteStore;
use crate::db::testing::{User, sweep};

/// Scans a library of three tracks, album X by B in two folders, the first of which holds the
/// artist's image and biography, and album Y by C, and returns its root and its store.
fn scanned(tmp: &Path) -> (PathBuf, SqliteStore) {
    let root = tmp.join("lib");
    let tags = |artist, album, track| {
        [
            (ItemKey::TrackArtist, artist),
            (ItemKey::AlbumTitle, album),
            (ItemKey::TrackNumber, track),
        ]
    };
    tagged(&root, "B/X/1.wav", &tags("B", "X", "1"));
    tagged(&root, "B/X2/2.wav", &tags("B", "X", "2"));
    tagged(&root, "C/Y/1.wav", &tags("C", "Y", "1"));
    fs::write(root.join("B/X/artist.txt"), "B's biography.").unwrap();
    fs::write(root.join("B/X/artist.png"), testing::PNG).unwrap();
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    (root, store)
}

/// Deletes the files of B/X/1 and C/Y/1 and scans, so their tracks go missing, then moves when
/// they did `ms` into the past.
fn missing_for(root: &Path, store: &SqliteStore, ms: i64) {
    fs::remove_file(root.join("B/X/1.wav")).unwrap();
    fs::remove_file(root.join("C/Y/1.wav")).unwrap();
    testing::scan_library(store, &testing::library(root));
    assert_eq!(count(store, "SELECT count(missing_since) FROM tracks"), 2);
    store
        .db()
        .write_blocking(move |tx| {
            tx.execute("UPDATE tracks SET missing_since = missing_since - ?1", [ms])
        })
        .unwrap();
}

/// Adds user 1's play of `track`, with its snapshot, and returns the row as stored.
fn played(store: &SqliteStore, track: i64) -> Vec<Value> {
    store
        .db()
        .write_blocking(move |tx| {
            User::new(1, "owner").insert(tx)?;
            tx.execute(
                "INSERT INTO plays (id, user_id, library_id, track_id, title, artists, \
                 artist_keys, album, album_title_key, album_artists_key, genre_keys, duration_us, \
                 started_at, listen_time_ms, ended, context, origin, updated_at) \
                 VALUES ('p', 1, 1, ?1, '1', '[\"C\"]', '[\"c\"]', 'Y', 'y', 'c', '[]', 1000, 0, \
                         1000, 'finished', '{}', 'manual', 0)",
                [track],
            )?;
            play(tx)
        })
        .unwrap()
}

/// The one play, as stored.
fn play(conn: &Connection) -> rusqlite::Result<Vec<Value>> {
    conn.query_row("SELECT * FROM plays", [], |r| {
        (0..r.as_ref().column_count()).map(|i| r.get(i)).collect()
    })
}

/// Tracks missing for longer than the window are purged with what they alone made: album Y and
/// artist C, B's image and biography, which only the purged track offered, and the image and
/// biography rows. Every change is in the feed, the tracks' deletion included, and a play of
/// a purged track keeps its snapshot (`requirements/scanning.md` §8.1).
#[test]
fn a_track_missing_past_the_window_is_purged_with_what_it_alone_made() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, store) = scanned(tmp.path());
    let purged: Vec<i64> = store
        .db()
        .read_blocking(|conn| {
            conn.prepare("SELECT id FROM tracks WHERE path IN ('B/X/1.wav', 'C/Y/1.wav')")?
                .query_map([], |r| r.get(0))?
                .collect()
        })
        .unwrap();
    missing_for(&root, &store, WINDOW_MS + 60_000);
    let before = snapshot(&store);
    let before_play = played(&store, purged[0]);

    sweep(store.db(), now_ms());

    assert_eq!(store.db().read_blocking(play).unwrap(), before_play);
    assert_eq!(count(&store, "SELECT count(*) FROM tracks"), 1);
    assert_eq!(scalar::<String>(&store, "SELECT group_concat(title) FROM albums").unwrap(), "X");
    assert_eq!(scalar::<String>(&store, "SELECT group_concat(name) FROM artists").unwrap(), "B");
    assert_eq!(count(&store, "SELECT count(image_id) + count(biography) FROM artists"), 0);
    assert_eq!(count(&store, "SELECT count(*) FROM images"), 0);
    assert_eq!(count(&store, "SELECT count(*) FROM biographies"), 0);
    check(&store, "the purge");
    assert_fed(&store, &before, "the purge");
    let deleted: Vec<i64> = store
        .db()
        .read_blocking(|conn| {
            conn.prepare(
                "SELECT entity_id FROM library_changes \
                 WHERE entity_type = 'track' AND op = 'delete' ORDER BY entity_id",
            )?
            .query_map([], |r| r.get(0))?
            .collect()
        })
        .unwrap();
    let mut purged = purged;
    purged.sort_unstable();
    assert_eq!(deleted, purged);
}

/// A track missing for 29 days is kept, and so is everything it makes, with nothing fed.
#[test]
fn a_track_missing_inside_the_window_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, store) = scanned(tmp.path());
    missing_for(&root, &store, 29 * 24 * 60 * 60 * 1000);
    let head = store.feed_head(1);

    sweep(store.db(), now_ms());

    assert_eq!(count(&store, "SELECT count(*) FROM tracks"), 3);
    assert_eq!(count(&store, "SELECT count(*) FROM albums"), 2);
    assert_eq!(count(&store, "SELECT count(*) FROM images"), 1);
    assert_eq!(count(&store, "SELECT count(*) FROM biographies"), 1);
    assert_eq!(store.feed_head(1), head);
}

/// Moves the missing marks and the roots' last whole walks `ms` into the past, as if that long
/// had passed since.
fn aged(store: &SqliteStore, ms: i64) {
    store
        .db()
        .write_blocking(move |tx| {
            tx.execute("UPDATE tracks SET missing_since = missing_since - ?1", [ms])?;
            tx.execute("UPDATE library_roots SET walked_at = walked_at - ?1", [ms])
        })
        .unwrap();
}

/// A track missing past the window is purged only once a scan of its whole root that started
/// after the window was up completes, still not finding it: not after the scan that marked it, nor
/// a scan of a folder, nor a scan of the root that suspends because the root lists empty, as an
/// unmounted share's does (`requirements/scanning.md` §7).
#[test]
fn a_track_missing_past_the_window_waits_for_a_later_scan_of_its_root() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, store) = scanned(tmp.path());
    missing_for(&root, &store, 0);
    aged(&store, WINDOW_MS + 60_000);
    let (lib, tracks) = (testing::library(&root), "SELECT count(*) FROM tracks");
    sweep(store.db(), now_ms());
    assert_eq!(count(&store, tracks), 3, "the scan that marked them started before the window");

    let folder = Scope::folder(&root, root.join("C"), Depth::Subtree);
    testing::scan_once(&store, &lib, Trigger::Manual, vec![folder]);
    sweep(store.db(), now_ms());
    assert_eq!(count(&store, tracks), 3, "a scan of a folder");

    let away = tmp.path().join("away");
    fs::rename(&root, &away).unwrap();
    fs::create_dir(&root).unwrap();
    assert_eq!(testing::scan_library(&store, &lib).state, ScanState::Suspended);
    sweep(store.db(), now_ms());
    assert_eq!(count(&store, tracks), 3, "a scan of the root listing empty");

    fs::remove_dir(&root).unwrap();
    fs::rename(&away, &root).unwrap();
    assert_eq!(testing::scan_library(&store, &lib).state, ScanState::Completed);
    sweep(store.db(), now_ms());
    assert_eq!(count(&store, tracks), 1);
}
