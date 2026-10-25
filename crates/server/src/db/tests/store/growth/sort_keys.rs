//! A changed sort key reaches the tracks that copy it, its album's or its artist's, at what those
//! tracks cost, not what the library holds (`design/database.md` §3).

use jewelcase_scanner::*;

use super::super::make;
use super::{cost, ids, track};

/// Retagging the sort keys of an album and its artist, whose 16 tracks then copy the new keys,
/// costs the same before and after 1,000 albums of other artists join the library, though the
/// planner's statistics were gathered over two tracks (`design/database.md` §4).
#[test]
fn a_changed_sort_key_costs_the_tracks_that_copy_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    let upserts = (0..2).map(|i| track(&root, "Plain", i)).collect();
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    store.db().write_blocking(|tx| tx.execute_batch("ANALYZE")).unwrap();
    let mine = |i: usize, sort: &str| {
        let mut rec = track(&root, "Mine", i);
        rec.tags.artists = vec!["Zed".to_owned()];
        rec.sort.album = sort.to_owned();
        rec.sort.artist = sort.to_owned();
        rec.sort.album_artist = sort.to_owned();
        rec
    };
    let upserts = (0..16).map(|i| mine(i, "mine")).collect();
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    let ids = ids(&store, "Mine");
    let retagged = |sort: &str| {
        let upserts = (0..16).map(|i| TrackRecord { id: Some(ids[i]), ..mine(i, sort) }).collect();
        Batch { upserts, ..Batch::default() }
    };

    let alone = cost(&store, retagged("first"));
    for start in (0..1000).step_by(250) {
        let upserts = (start..start + 250)
            .map(|i| {
                let mut rec = track(&root, &format!("Other {i}"), i);
                rec.tags.artists = vec![format!("Other {i}")];
                rec.sort.artist = format!("other {i}");
                rec.sort.album_artist = format!("other {i}");
                rec
            })
            .collect();
        store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    }
    let among = cost(&store, retagged("second"));
    // Reading every track, even once, would take at least 1,000 steps more.
    assert!(among < alone + 500, "{among} steps against {alone}");
}
