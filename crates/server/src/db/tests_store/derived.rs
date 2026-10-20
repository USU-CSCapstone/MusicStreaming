//! The derived values after each step of a long, mixed run of scanner writes, against a
//! recompute from scratch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use jewelcase_scanner::*;

use super::recompute::{assert_fed, check, snapshot};
use super::record;
use crate::db::catalog::{self, Touched};
use crate::db::store::SqliteStore;
use crate::db::{Database, libraries};

/// A small seeded generator (Marsaglia's xorshift64), so every run makes the same sequence.
struct Rng(u64);

impl Rng {
    /// The next number in the sequence.
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// One of `items`.
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() % items.len() as u64) as usize]
    }

    /// Whether an event with a `percent` chance happens.
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

const ARTISTS: &[&str] = &["Alpha", "alpha", " ALPHA ", "Beta", "beta", "Gamma", "Ünï", "ünï"];
const ALBUMS: &[&str] = &["Red", "red", " RED", "Blue", "Green"];
const GENRES: &[&str] = &["Rock", "rock", " Rock ", "Jazz", "jazz", "Hip Hop", "Hip-Hop", "Pop"];
const SORTS: &[&str] = &["", "alpha", "beta", "zeta"];

/// A record of the file at `path` under `root`, tagged from `rng`: up to two artists and three
/// genres in several spellings, sometimes album artists or a compilation, sometimes no album.
fn random(rng: &mut Rng, root: &Path, path: PathBuf) -> TrackRecord {
    let names = |rng: &mut Rng, pool: &[&str], most: u64| -> Vec<String> {
        (0..rng.next() % (most + 1)).map(|_| rng.pick(pool).to_string()).collect()
    };
    let mut rec = TrackRecord { path, ..record(root, 0) };
    rec.tags.artists = names(rng, ARTISTS, 2);
    rec.tags.album_artists = if rng.chance(30) { names(rng, ARTISTS, 2) } else { Vec::new() };
    rec.tags.compilation = rng.chance(15);
    rec.tags.album = rng.chance(80).then(|| rng.pick(ALBUMS).to_string());
    rec.tags.genres = names(rng, GENRES, 3);
    rec.tags.disc_number = *rng.pick(&[None, Some(1), Some(2)]);
    rec.tags.disc_total = *rng.pick(&[None, Some(2)]);
    rec.tags.track_total = *rng.pick(&[None, Some(3), Some(12)]);
    rec.tags.release_date = PartialDate::parse(rng.pick(&["1999", "2001-05"]));
    rec.sort.artist = rng.pick(SORTS).to_string();
    rec.properties.duration_ms = 1000 * (1 + rng.next() % 300);
    rec
}

/// Every track by its file, with its ID, library, and whether it is missing.
fn tracks(store: &SqliteStore) -> BTreeMap<PathBuf, (u64, i64, bool)> {
    let rows: Vec<(String, String, i64, i64, bool)> = store
        .db()
        .read_blocking(|conn| {
            conn.prepare(
                "SELECT r.path, t.path, t.id, t.library_id, t.missing_since IS NOT NULL \
                 FROM tracks t JOIN library_roots r ON r.id = t.root_id",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect()
        })
        .unwrap();
    let file = |root: String, rel: String| Path::new(&root).join(rel);
    rows.into_iter()
        .map(|(root, rel, id, lib, gone)| (file(root, rel), (id as u64, lib, gone)))
        .collect()
}

#[test]
fn derived_values_equal_a_recompute_after_every_change() {
    let tmp = tempfile::tempdir().unwrap();
    let [a, b, c]: [PathBuf; 3] = ["a", "b", "c"].map(|r| tmp.path().join(r));
    let db = Arc::new(Database::open(&tmp.path().join("jewelcase.db")).unwrap());
    let roots = (a.clone(), b.clone(), c.clone());
    db.write_blocking(move |tx| {
        libraries::create(tx, Some(1), "Two roots", &[&roots.0, &roots.1], &[])?;
        libraries::create(tx, Some(2), "Other", &[&roots.2], &[]).map(drop)
    })
    .unwrap();
    let store = SqliteStore::new(db).unwrap();
    let (one, two): (LibraryId, LibraryId) = ("1".into(), "2".into());
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut ran = [0; 10];

    for scan in 1..=400 {
        let before = snapshot(&store);
        let known = tracks(&store);
        // Library 1's tracks, present or missing, in path order so the run repeats exactly.
        let of_one = |gone: bool| -> Vec<(PathBuf, u64)> {
            let ones = known.iter().filter(|(_, t)| t.1 == 1 && t.2 == gone);
            ones.map(|(path, t)| (path.clone(), t.0)).collect()
        };
        let (present, missing) = (of_one(false), of_one(true));
        let id = |path: &Path| known.get(path).map(|t| t.0);
        let op = rng.next() % 10;
        match op {
            // New files and changed ones on both roots, with a returning file in the same batch.
            0..=3 => {
                let mut upserts: Vec<TrackRecord> = Vec::new();
                for _ in 0..1 + rng.next() % 4 {
                    let root = rng.pick(&[&a, &b]).as_path();
                    let path = root.join(format!("{}.flac", rng.next() % 12));
                    if upserts.iter().all(|u| u.path != path) {
                        upserts.push(TrackRecord { id: id(&path), ..random(&mut rng, root, path) });
                    }
                }
                let returned =
                    missing.first().filter(|(p, _)| upserts.iter().all(|u| &u.path != p));
                let returned = returned.map(|(_, id)| *id).into_iter().collect();
                store.apply(&one, Batch { upserts, returned, ..Batch::new(scan) }).unwrap();
            }
            // A file moved to the other root, retagged on the way.
            4 if !present.is_empty() => {
                let (from, id) = rng.pick(&present).clone();
                let to = if from.starts_with(&a) { &b } else { &a };
                let path = to.join(format!("moved {scan}.flac"));
                let rec = TrackRecord { id: Some(id), ..random(&mut rng, to, path) };
                store.apply(&one, Batch { upserts: vec![rec], ..Batch::new(scan) }).unwrap();
            }
            // A scan of one root that sees about half of its files, so the rest go missing.
            5 => {
                let root = rng.pick(&[&a, &b]).as_path();
                let on_root = present.iter().filter(|(p, _)| p.starts_with(root));
                let touched = on_root.filter(|_| rng.chance(50)).map(|(_, id)| *id).collect();
                store.apply(&one, Batch { touched, ..Batch::new(scan) }).unwrap();
                store.mark_missing_unseen(&one, &Scope::root(root), scan, &[]);
            }
            // Missing files coming back unchanged.
            6 => {
                let returned = missing.iter().take(3).map(|(_, id)| *id).collect();
                store.apply(&one, Batch { returned, ..Batch::new(scan) }).unwrap();
            }
            // A missing file back with new tags.
            7 if !missing.is_empty() => {
                let (path, id) = rng.pick(&missing).clone();
                let root = if path.starts_with(&a) { &a } else { &b };
                let rec = TrackRecord { id: Some(id), ..random(&mut rng, root, path) };
                store.apply(&one, Batch { upserts: vec![rec], ..Batch::new(scan) }).unwrap();
            }
            // The other library, with the same names, which must count apart.
            8 => {
                let path = c.join(format!("{}.flac", rng.next() % 6));
                let rec = TrackRecord { id: id(&path), ..random(&mut rng, &c, path) };
                store.apply(&two, Batch { upserts: vec![rec], ..Batch::new(scan) }).unwrap();
            }
            // A track deleted outright, which the scanner itself never does.
            9 if !present.is_empty() => {
                let id = rng.pick(&present).1 as i64;
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
            _ => continue,
        }
        let step = format!("scan {scan}, operation {op}");
        check(&store, &step);
        assert_fed(&store, &before, &step);
        ran[op as usize] += 1;
    }
    // Moving, retagging a missing file and deleting run only when there is a track to choose, so
    // check that every kind of change ran often enough to matter.
    assert!(ran.iter().all(|&n| n >= 10), "{ran:?}");

    // Deleting a library cascades through its rows, whose triggers must leave the other
    // library's counts alone.
    let delete =
        |tx: &rusqlite::Transaction<'_>| tx.execute("DELETE FROM libraries WHERE id = 2", []);
    assert_eq!(store.db().write_blocking(delete).unwrap(), 1);
    check(&store, "deleting library 2");
}
