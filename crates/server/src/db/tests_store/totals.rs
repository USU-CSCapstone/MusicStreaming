//! Library totals, which triggers keep, against what a scan of the tracks gives.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use jewelcase_scanner::testing::{InspectStore, duration_ms};
use jewelcase_scanner::*;
use rusqlite::Result;

use super::{count, make, record, scalar};
use crate::db::store::SqliteStore;
use crate::db::{Database, libraries};

/// Each library's stored totals beside the same totals counted from its tracks and credits.
const TOTALS: &str = "SELECT l.id, l.track_count, l.album_count, l.artist_count, l.duration_us, \
     (SELECT COUNT(*) FROM tracks t WHERE t.library_id = l.id), \
     (SELECT COUNT(DISTINCT album_id) FROM tracks t WHERE t.library_id = l.id), \
     (SELECT COUNT(*) FROM (SELECT artist_id FROM track_artists WHERE library_id = l.id \
      UNION SELECT artist_id FROM track_album_artists WHERE library_id = l.id)), \
     (SELECT COALESCE(SUM(duration_us), 0) FROM tracks t WHERE t.library_id = l.id) \
     FROM libraries l ORDER BY l.id";

/// Asserts that every library's totals equal its track scan, after `step`.
fn assert_totals_match_tracks(store: &SqliteStore, step: &str) {
    let rows: Vec<(i64, [i64; 4], [i64; 4])> = store
        .db()
        .read_blocking(|conn| {
            conn.prepare(TOTALS)?
                .query_map([], |r| {
                    Ok((
                        r.get(0)?,
                        [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?],
                        [r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?],
                    ))
                })?
                .collect::<Result<_>>()
        })
        .unwrap();
    assert_eq!(rows.len(), 2);
    for (library, stored, scanned) in rows {
        assert_eq!(
            stored, scanned,
            "library {library} after {step}: tracks, albums, artists, duration"
        );
    }
}

/// Record `i` under `root` in album `album`, lasting `i + 1` seconds, so every duration differs.
fn track(root: &Path, i: usize, album: &str) -> TrackRecord {
    let mut rec = record(root, i);
    rec.tags.album = Some(album.into());
    rec.properties.duration_ms = (i as u64 + 1) * 1000;
    rec
}

/// Track `i` of a various-artists compilation: its own artist and no album artist.
fn compilation_track(root: &Path, i: usize) -> TrackRecord {
    let mut rec = track(root, i, "Hits");
    rec.tags.artists = vec![format!("Singer {i}")];
    rec.tags.album_artists.clear();
    rec.tags.compilation = true;
    rec
}

/// A file with no tags at all, which lands on the unknown album and artist.
fn untagged(root: &Path, i: usize) -> TrackRecord {
    let mut rec = track(root, i, "");
    rec.tags = Default::default();
    rec
}

/// `rec` as an update of library 1's track at its path.
fn update(store: &SqliteStore, rec: TrackRecord) -> TrackRecord {
    TrackRecord { id: Some(id_at(store, &rec.path)), ..rec }
}

/// The ID of library 1's track at `path`.
fn id_at(store: &SqliteStore, path: &Path) -> TrackId {
    let tracks = store.inspect_tracks(1);
    tracks.iter().find(|t| t.path == path).map(|t| t.id).unwrap()
}

/// Applies `upserts` to library `lib`.
fn upsert(store: &SqliteStore, lib: LibraryId, upserts: Vec<TrackRecord>) {
    store.apply(lib, Batch { upserts, ..Batch::default() }).unwrap();
}

#[test]
fn library_totals_equal_a_track_scan() {
    let tmp = tempfile::tempdir().unwrap();
    let [a, b, other]: [PathBuf; 3] = ["a", "b", "other"].map(|r| tmp.path().join(r));
    let db = Arc::new(Database::open(&tmp.path().join("jewelcase.db")).unwrap());
    let roots = (a.clone(), b.clone(), other.clone());
    db.write_blocking(move |tx| {
        libraries::create(tx, Some(1), "Two roots", &[&roots.0, &roots.1], &[])?;
        libraries::create(tx, Some(2), "Other", &[&roots.2], &[]).map(drop)
    })
    .unwrap();
    let store = SqliteStore::new(db).unwrap();

    // One album split across both roots, one on each, a compilation, an album of its own, and
    // an untagged file; library 2 has an album too, which library 1's totals must not count.
    let mut first: Vec<TrackRecord> = (0..4).map(|i| track(&a, i, "Split")).collect();
    first.extend((4..7).map(|i| track(&b, i, "Split")));
    first.extend((7..9).map(|i| track(&a, i, "Only A")));
    first.extend((9..11).map(|i| track(&b, i, "Only B")));
    first.extend((11..14).map(|i| compilation_track(&a, i)));
    first.push(track(&b, 14, "Solo"));
    first.push(untagged(&a, 15));
    upsert(&store, 1, first);
    upsert(&store, 2, (0..3).map(|i| track(&other, i, "Elsewhere")).collect());
    assert_totals_match_tracks(&store, "the first upserts");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM albums WHERE library_id = 1"), 6);

    // Retags: the solo track joins another album, emptying its own; the untagged file gains an
    // album; a compilation track leaves the compilation; one track moves to the other root.
    let retags = vec![
        update(&store, track(&b, 14, "Only B")),
        update(&store, track(&a, 15, "Only A")),
        update(&store, track(&a, 13, "Split")),
        TrackRecord { id: Some(id_at(&store, &track(&b, 6, "").path)), ..track(&a, 6, "Split") },
    ];
    upsert(&store, 1, retags);
    assert_totals_match_tracks(&store, "retags and a move");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM albums WHERE library_id = 1"), 4);

    // Everything on root b goes missing except one track, and stays counted: two tracks gone
    // from their folder, and the two in a folder gone from its parent.
    let on_b = |i| id_at(&store, &track(&b, i, "").path);
    let missing = vec![on_b(4), on_b(5)];
    let vanished = vec![b.join("Artist 0/Album 1")];
    assert_eq!(store.apply(1, Batch { missing, vanished, ..Batch::default() }).unwrap(), 4);
    assert_totals_match_tracks(&store, "tracks going missing");

    // Two come back, one is retagged into the compilation while missing, and one stays missing.
    let returned = [5, 10].map(|i| id_at(&store, &track(&b, i, "").path)).to_vec();
    store.apply(1, Batch { returned, ..Batch::default() }).unwrap();
    upsert(&store, 1, vec![update(&store, compilation_track(&b, 14))]);
    assert_totals_match_tracks(&store, "restores and retagging a missing track");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM tracks WHERE missing_since IS NOT NULL"), 1);
}

/// A track whose headers claim an impossible length lasts nothing, so the totals of its album
/// and library stay those of the tracks beside it.
#[test]
fn a_track_claiming_an_impossible_length_adds_nothing_to_the_totals() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    let mut endless = track(&root, 0, "Long");
    endless.properties.duration_ms = duration_ms(Duration::MAX);
    upsert(&store, 1, vec![endless, track(&root, 1, "Long"), track(&root, 2, "Short")]);
    let durations = |sql: &'static str| scalar::<i64>(&store, sql).unwrap();
    assert_eq!(durations("SELECT duration_us FROM libraries"), 5_000_000);
    assert_eq!(durations("SELECT duration_us FROM albums WHERE title = 'Long'"), 2_000_000);
    assert_eq!(durations("SELECT duration_us FROM albums WHERE title = 'Short'"), 3_000_000);
}
