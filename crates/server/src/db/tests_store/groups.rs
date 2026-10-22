//! How an album resolves from its tracks: groups that hold several tracks, as each kind of change
//! moves tracks in and out of them, against a recompute from scratch; the most-used release type;
//! and the order of the album artists.

use std::path::Path;

use jewelcase_scanner::*;

use super::recompute::check;
use super::{make, record};
use crate::db::catalog::{self, Touched};
use crate::db::store::SqliteStore;

/// Track `i` of album "Shared" under `root`, tagged like the album's other tracks, so they share
/// one group.
fn track(root: &Path, i: usize) -> TrackRecord {
    let mut rec = record(root, i);
    rec.path = root.join(format!("Shared/{i:02}.flac"));
    rec.tags.album = Some("Shared".to_owned());
    rec.tags.track_number = Some(i as u32 + 1);
    rec
}

/// The IDs of album "Shared"'s tracks, in path order.
fn ids(store: &SqliteStore) -> Vec<TrackId> {
    store
        .db()
        .read_blocking(|conn| {
            conn.prepare("SELECT id FROM tracks WHERE path LIKE 'Shared/%' ORDER BY path")?
                .query_map([], |r| r.get(0))?
                .collect()
        })
        .unwrap()
}

#[test]
fn groups_of_several_tracks_follow_every_change() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    let upserts = (0..6).map(|i| track(&root, i)).collect();
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    check(&store, "adding six tracks to one group");
    let ids = ids(&store);

    store.apply(1, Batch { missing: ids[..3].to_vec(), ..Batch::default() }).unwrap();
    check(&store, "marking three of them missing");
    store.apply(1, Batch { returned: vec![ids[0]], ..Batch::default() }).unwrap();
    check(&store, "one returning");

    // One reread longer, and one retagged into a group of its own.
    let mut longer = TrackRecord { id: Some(ids[3]), ..track(&root, 3) };
    longer.properties.duration_ms += 1000;
    let mut retagged = TrackRecord { id: Some(ids[4]), ..track(&root, 4) };
    retagged.tags.release_date = PartialDate::parse("2001");
    store.apply(1, Batch { upserts: vec![longer, retagged], ..Batch::default() }).unwrap();
    check(&store, "rereading two of them changed");

    delete(&store, ids[5]);
    check(&store, "deleting a present one");
    delete(&store, ids[1]);
    check(&store, "deleting a missing one");
}

/// Deletes track `id` outright, as the purge of tracks long missing does, and recomputes what it
/// pointed at.
fn delete(store: &SqliteStore, id: TrackId) {
    store
        .db()
        .write_blocking(move |tx| {
            let mut touched = Touched::default();
            touched.track(tx, 1, id)?;
            tx.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
            catalog::recompute(tx, 1, &touched, 0)
        })
        .unwrap();
}

#[test]
fn an_album_takes_its_most_used_release_type_ties_broken_by_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    let albums: [(&str, &[&str]); 3] = [
        ("Mostly EP", &["ep", "ep", "single"]),
        ("Mostly single", &["single", "single", "ep"]),
        ("Tied", &["single", "ep"]),
    ];
    let mut upserts = Vec::new();
    for (album, types) in albums {
        for (i, kind) in types.iter().enumerate() {
            let mut rec = record(&root, i);
            rec.path = root.join(format!("{album}/{i}.flac"));
            rec.tags.album = Some(album.to_owned());
            rec.tags.release_type = Some(kind.to_string());
            upserts.push(rec);
        }
    }
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    let types: Vec<(String, String)> = store
        .db()
        .read_blocking(|conn| {
            conn.prepare("SELECT title, type FROM albums ORDER BY title")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect()
        })
        .unwrap();
    let expected = [("Mostly EP", "ep"), ("Mostly single", "single"), ("Tied", "ep")];
    assert_eq!(types, expected.map(|(a, t)| (a.to_owned(), t.to_owned())));
}

#[test]
fn the_album_artists_are_in_the_order_its_first_track_lists_them() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let store = make(&root);
    // The first track has the highest ID, and the others list the album artists the other way
    // round: the second, track 1 of a later disc, and one with no number, last on its disc.
    let credited = |i: usize, id: TrackId, album_artists: [&str; 2]| {
        let mut rec = track(&root, i);
        rec.id = Some(id);
        rec.tags.album_artists = album_artists.map(str::to_owned).to_vec();
        rec
    };
    let mut later_disc = credited(2, 50, ["Alpha", "Beta"]);
    later_disc.tags.disc_number = Some(2);
    later_disc.tags.track_number = Some(1);
    let mut unnumbered = credited(3, 10, ["Alpha", "Beta"]);
    unnumbered.tags.track_number = None;
    let first = credited(0, 200, ["Beta", "Alpha"]);
    let upserts = vec![first, credited(1, 100, ["Alpha", "Beta"]), later_disc, unnumbered];
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    check(&store, "the four tracks");
    let names: Vec<String> = store
        .db()
        .read_blocking(|conn| {
            conn.prepare(
                "SELECT a.name FROM album_artists aa JOIN artists a ON a.id = aa.artist_id \
                 ORDER BY aa.position",
            )?
            .query_map([], |r| r.get(0))?
            .collect()
        })
        .unwrap();
    assert_eq!(names, ["Beta", "Alpha"]);
}
