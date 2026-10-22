//! A batch costs what it changes, not what the album it changes holds (`design/database.md` §3).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use jewelcase_scanner::*;

use super::{make, record};
use crate::db::store::SqliteStore;

/// Track `i` of album `album` under `root`, with embedded art and a label, by one artist.
fn track(root: &Path, album: &str, i: usize) -> TrackRecord {
    let mut rec = record(root, i);
    rec.path = root.join(format!("{album}/{i:05}.flac"));
    rec.tags.album = Some(album.to_owned());
    rec.tags.artists = vec!["Artist".to_owned()];
    rec.tags.album_artists = Vec::new();
    rec.tags.labels = vec!["Label".to_owned()];
    rec.sort.album = album.to_lowercase();
    rec.artwork = Some(Artwork {
        source: ArtworkSource::Embedded,
        info: ImageInfo { hash: vec![1], width: 1, height: 1 },
    });
    rec
}

/// How many virtual-machine steps SQLite takes applying `batch` to library 1, triggers' included,
/// counted by a progress handler called at every step on the writer connection `apply` uses.
fn cost(store: &SqliteStore, batch: Batch) -> u64 {
    let steps = Arc::new(AtomicU64::new(0));
    let counter = steps.clone();
    let count = move || {
        counter.fetch_add(1, Ordering::Relaxed);
        false
    };
    let db = store.db();
    db.write_blocking(move |tx| tx.progress_handler(1, Some(count))).unwrap();
    store.apply(1, batch).unwrap();
    db.write_blocking(|tx| tx.progress_handler(0, None::<fn() -> bool>)).unwrap();
    steps.load(Ordering::Relaxed)
}

/// The IDs of album `album`'s tracks, in path order.
fn ids(store: &SqliteStore, album: &'static str) -> Vec<TrackId> {
    store
        .db()
        .read_blocking(move |conn| {
            conn.prepare(
                "SELECT t.id FROM tracks t JOIN albums a ON a.id = t.album_id \
                 WHERE a.title = ?1 ORDER BY t.path",
            )?
            .query_map([album], |r| r.get(0))?
            .collect()
        })
        .unwrap()
}

#[test]
fn a_batch_into_a_large_album_costs_what_one_into_a_small_album_does() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    for start in (0..1000).step_by(250) {
        let upserts = (start..start + 250).map(|i| track(&root, "Large", i)).collect();
        store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    }
    let upserts = (0..16).map(|i| track(&root, "Small", i)).collect();
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();

    // Each change, made to 16 tracks of each album: adding them, rereading them changed, and
    // marking them missing.
    let added = |album: &str| {
        let upserts = (5000..5016).map(|i| track(&root, album, i)).collect();
        Batch { upserts, ..Batch::default() }
    };
    let changed = |album: &str, ids: &[TrackId]| {
        let upserts = (0..16)
            .map(|i| TrackRecord { id: Some(ids[i]), size: 7, ..track(&root, album, i) })
            .collect();
        Batch { upserts, ..Batch::default() }
    };
    let missing = |ids: &[TrackId]| Batch { missing: ids[..16].to_vec(), ..Batch::default() };
    let (large, small) = (ids(&store, "Large"), ids(&store, "Small"));
    let costs = [
        ("added", cost(&store, added("Large")), cost(&store, added("Small"))),
        ("changed", cost(&store, changed("Large", &large)), cost(&store, changed("Small", &small))),
        ("missing", cost(&store, missing(&large)), cost(&store, missing(&small))),
    ];
    for (change, large, small) in costs {
        // Reading every track of the large album, even once, would take at least 1,000 steps more.
        assert!(large < small + 500, "{change}: {large} steps against {small}");
    }
}

#[test]
fn emptying_a_group_costs_what_keeping_it_does() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    // An album of 1,000 groups, one per track total: tracks 0 to 999 each in a group of their
    // own, but for the first 16, which share theirs with tracks 1,000 to 1,015.
    let scattered = |i: usize| {
        let mut rec = track(&root, "Scattered", i);
        rec.tags.track_total = Some((i % 1000) as u32);
        rec
    };
    for start in (0..1000).step_by(250) {
        let upserts = (start..start + 250).map(scattered).collect();
        store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    }
    let upserts = (1000..1016).map(scattered).collect();
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();

    // Marking a track missing moves it out of its group and back in, which empties the group
    // only when the track was alone in it.
    let ids = ids(&store, "Scattered");
    let kept = cost(&store, Batch { missing: ids[..16].to_vec(), ..Batch::default() });
    let emptied = cost(&store, Batch { missing: ids[16..32].to_vec(), ..Batch::default() });
    // Reading the album's groups for each group emptied would take at least 16,000 steps more.
    assert!(emptied < kept + 500, "{emptied} steps against {kept}");
}
