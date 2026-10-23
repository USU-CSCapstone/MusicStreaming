//! An album's track total: the sum of its discs' tagged totals, as the spec's `trackTotal` gives
//! it, and the type inferred from it (`design/database.md` §3).

use std::sync::Arc;

use jewelcase_scanner::*;
use rusqlite::Result;

use super::recompute::check;
use super::record;
use crate::db::store::SqliteStore;
use crate::db::{Database, libraries};

/// Each album's title and stored track total, by title.
fn totals(store: &SqliteStore) -> Vec<(String, Option<i64>)> {
    store
        .db()
        .read_blocking(|conn| {
            conn.prepare("SELECT title, track_total FROM albums ORDER BY title")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_>>()
        })
        .unwrap()
}

/// One disc's total is the album's; two discs' totals add up, each the largest its tracks tag;
/// a disc with no total adds nothing; and an album with none has none.
#[test]
fn an_albums_track_total_sums_its_discs() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let db = Arc::new(Database::open(&tmp.path().join("jewelcase.db")).unwrap());
    let made = root.clone();
    db.write_blocking(move |tx| libraries::create(tx, Some(1), "Music", &[&made], &[]).map(drop))
        .unwrap();
    let store = SqliteStore::new(db).unwrap();
    let layouts: [(&str, Option<u32>, Option<u32>); 8] = [
        ("One", Some(1), Some(10)),
        ("One", Some(1), Some(10)),
        ("Two", Some(1), Some(3)),
        ("Two", Some(1), Some(2)),
        ("Two", Some(2), Some(4)),
        ("Part", Some(1), Some(5)),
        ("Part", Some(2), None),
        ("None", None, None),
    ];
    let upserts = (0..).zip(layouts).map(|(i, (album, disc_number, track_total))| {
        let mut rec = record(&root, i);
        rec.tags.album = Some(album.into());
        rec.tags.disc_number = disc_number;
        rec.tags.track_total = track_total;
        rec
    });
    store.apply(1, Batch { upserts: upserts.collect(), ..Batch::default() }).unwrap();
    check(&store, "the upserts");
    let expected = [("None", None), ("One", Some(10)), ("Part", Some(5)), ("Two", Some(7))];
    assert_eq!(totals(&store), expected.map(|(title, total)| (title.to_owned(), total)));
}

/// An album's type follows its summed total, so two complete discs of five tracks are an album,
/// though either disc alone would be an EP (`requirements/albums.md` §3).
#[test]
fn a_double_album_of_two_short_discs_is_an_album() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let db = Arc::new(Database::open(&tmp.path().join("jewelcase.db")).unwrap());
    let made = root.clone();
    db.write_blocking(move |tx| libraries::create(tx, Some(1), "Music", &[&made], &[]).map(drop))
        .unwrap();
    let store = SqliteStore::new(db).unwrap();
    let upserts = (0..10).map(|i| {
        let mut rec = record(&root, i);
        rec.tags.album = Some("Double".into());
        rec.tags.disc_number = Some(1 + i as u32 / 5);
        rec.tags.track_total = Some(5);
        rec
    });
    store.apply(1, Batch { upserts: upserts.collect(), ..Batch::default() }).unwrap();
    check(&store, "the upserts");
    let album = store
        .db()
        .read_blocking(|conn| {
            conn.query_row("SELECT type, track_total FROM albums", [], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
        })
        .unwrap();
    assert_eq!(album, ("album".to_owned(), 10));
}
