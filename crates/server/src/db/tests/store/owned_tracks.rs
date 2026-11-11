//! The tracks on an artist's albums, counted from the tracks themselves (what the shared track
//! listing reads on a device) against the server's album groups.

use jewelcase_scanner::testing::{self, ItemKey};

use super::{make, tagged};

#[test]
fn an_artists_owned_tracks_are_the_same_from_tracks_as_from_album_groups() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    // A two-disc album by two artists, one track of which is retagged into a group of its own,
    // an album by one of them alone, and a track on it that goes missing.
    let multi = |disc, number, date| {
        [
            (ItemKey::AlbumTitle, "Multi"),
            (ItemKey::AlbumArtist, "Alpha; Beta"),
            (ItemKey::DiscNumber, disc),
            (ItemKey::TrackNumber, number),
            (ItemKey::RecordingDate, date),
        ]
    };
    for (rel, disc, number, date) in [
        ("multi/1-1.wav", "1", "1", "2001"),
        ("multi/1-2.wav", "1", "2", "2001"),
        ("multi/1-3.wav", "1", "3", "2002"),
        ("multi/2-1.wav", "2", "1", "2001"),
    ] {
        tagged(&root, rel, &multi(disc, number, date));
    }
    for (rel, number) in [("solo/1.wav", "1"), ("solo/2.wav", "2")] {
        tagged(
            &root,
            rel,
            &[
                (ItemKey::AlbumTitle, "Solo"),
                (ItemKey::AlbumArtist, "Alpha"),
                (ItemKey::TrackNumber, number),
            ],
        );
    }
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    store
        .db()
        .write_blocking(|tx| {
            tx.execute(
                "UPDATE tracks SET missing_since = 1 WHERE id = (SELECT min(id) FROM tracks \
                 WHERE album_id = (SELECT id FROM albums WHERE title = 'Solo'))",
                [],
            )
            .map(drop)
        })
        .unwrap();

    let owned = |expression: &'static str| -> Vec<(i64, i64)> {
        store
            .db()
            .read_blocking(move |conn| {
                conn.prepare(&format!("SELECT ar.id, {expression} FROM artists ar ORDER BY ar.id"))?
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            })
            .unwrap()
    };
    let groups = "(SELECT ifnull(sum(g.track_count), 0) FROM album_artists aa \
         JOIN album_groups g ON g.album_id = aa.album_id WHERE aa.artist_id = ar.id)";
    let counts = jewelcase_catalog::tracks::OWNED_TRACKS;
    let (from_groups, from_counts) = (owned(groups), owned(counts));
    assert_eq!(from_counts, from_groups);
    let totals: Vec<i64> = from_counts.iter().map(|&(_, n)| n).filter(|&n| n > 0).collect();
    assert_eq!(totals.iter().sum::<i64>(), 4 + 6, "Alpha and Beta own Multi; Alpha owns Solo too");
    let groups_of_multi = super::count(
        &store,
        "SELECT count(*) FROM album_groups WHERE album_id = (SELECT id FROM albums WHERE title = 'Multi')",
    );
    assert!(groups_of_multi >= 3, "the album's tracks are in several groups: {groups_of_multi}");
}
