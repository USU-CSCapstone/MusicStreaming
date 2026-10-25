//! The counts that follow an artist's credits (its tracks and credits on each album) cost what a
//! change touches, not what the artist or the album holds (`design/database.md` §3).

use std::path::Path;

use jewelcase_scanner::*;

use super::super::make;
use super::{cost, ids, steps, track};
use crate::db::catalog::{self, Touched};
use crate::db::store::SqliteStore;

/// Track `i` of album `album` by `owner`, under the owner's own sort key, crediting `guest` on it
/// too when there is one.
fn credited(root: &Path, owner: &str, album: &str, guest: Option<&str>, i: usize) -> TrackRecord {
    let mut rec = track(root, album, i);
    rec.path = root.join(format!("{owner}/{album}/{i:05}.flac"));
    rec.tags.artists = std::iter::once(owner).chain(guest).map(str::to_owned).collect();
    rec.tags.album_artists = vec![owner.to_owned()];
    rec.sort.artist = owner.to_lowercase();
    rec.sort.album_artist = owner.to_lowercase();
    rec
}

/// How many steps deleting `tracks` takes, with the recompute of what they counted toward, as the
/// sweep's purge does it.
fn deleted(store: &SqliteStore, tracks: Vec<TrackId>) -> u64 {
    steps(store, || {
        let delete = move |tx: &rusqlite::Transaction<'_>| {
            let mut touched = Touched::default();
            for &id in &tracks {
                touched.track(tx, 1, id)?;
                tx.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
            }
            catalog::recompute(tx, 1, &touched, 0)
        };
        store.db().write_blocking(delete).unwrap();
    })
}

/// Each change to 16 tracks of an artist with 2,000 credits costs what it does to one with 32.
#[test]
fn a_batch_for_a_large_artist_costs_what_one_for_a_small_artist_does() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    // Large owns an album of 1,000 tracks and appears on 100 albums of 10 tracks; Small owns an
    // album of 16 and appears on one.
    for start in (0..1000).step_by(250) {
        let upserts = (start..start + 250).map(|i| credited(&root, "Large", "Mine L", None, i));
        store.apply(1, Batch { upserts: upserts.collect(), ..Batch::default() }).unwrap();
        let upserts = (start..start + 250).map(|i| {
            let host = format!("Host {}", i / 10);
            credited(&root, &host, &host, Some("Large"), i)
        });
        store.apply(1, Batch { upserts: upserts.collect(), ..Batch::default() }).unwrap();
    }
    let upserts = (0..16)
        .map(|i| credited(&root, "Small", "Mine S", None, i))
        .chain((0..16).map(|i| credited(&root, "Host S", "Host S", Some("Small"), i)));
    store.apply(1, Batch { upserts: upserts.collect(), ..Batch::default() }).unwrap();

    // Each change, made to 16 tracks of each artist: adding them to its own album, adding them on
    // a new album that credits it as a guest, moving them to a new album of its own, and deleting
    // them from its own album and from the one it is a guest on.
    let added = |artist: &str, album: &str, guest: Option<&str>| {
        let upserts = (5000..5016).map(|i| credited(&root, artist, album, guest, i)).collect();
        Batch { upserts, ..Batch::default() }
    };
    let moved = |artist: &str, album: &str, ids: &[TrackId]| {
        let upserts = (0..16)
            .map(|i| TrackRecord { id: Some(ids[i]), ..credited(&root, artist, album, None, i) })
            .collect();
        Batch { upserts, ..Batch::default() }
    };
    let (large, small) = (ids(&store, "Mine L"), ids(&store, "Mine S"));
    let costs = [
        (
            "owned",
            cost(&store, added("Large", "Mine L", None)),
            cost(&store, added("Small", "Mine S", None)),
        ),
        (
            "featured",
            cost(&store, added("New L", "New L", Some("Large"))),
            cost(&store, added("New S", "New S", Some("Small"))),
        ),
        (
            "moved",
            cost(&store, moved("Large", "Moved L", &large)),
            cost(&store, moved("Small", "Moved S", &small)),
        ),
        (
            "deleted owned",
            deleted(&store, large[16..32].to_vec()),
            deleted(&store, ids(&store, "Mine S")[..16].to_vec()),
        ),
        (
            "deleted featured",
            deleted(&store, ids(&store, "New L")),
            deleted(&store, ids(&store, "New S")),
        ),
    ];
    for (change, large, small) in costs {
        // Reading every credit of the large artist even once would take 1,000 steps more.
        assert!(large < small + 500, "{change}: {large} steps against {small}");
    }
}
