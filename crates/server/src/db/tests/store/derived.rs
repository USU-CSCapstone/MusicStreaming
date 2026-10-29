//! The derived values after each step of a long, mixed run of scanner writes, against a
//! recompute from scratch.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use jewelcase_scanner::*;

use super::recompute::{assert_fed, check, snapshot};
use super::record;
use crate::db::catalog::{self, Touched};
use crate::db::libraries;
use crate::db::store::SqliteStore;
use crate::db::testing::open;

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
const GENRES: &[&str] =
    &["Rock", "rock", " Rock ", "Jazz", "jazz", "Hip Hop", "Hip-Hop", "Pop", "A Cappella"];
const SORTS: &[&str] = &["", "alpha", "beta", "zeta"];
const LABELS: &[&str] = &["Sub Pop", "Domino", "Matador"];
const TYPES: &[&str] = &["ep", "single", "album; compilation"];

/// A record of the file at `path` under `root`, tagged from `rng`: up to two artists, three
/// genres and two moods in several spellings, the moods from the genres' names, up to two
/// labels, sometimes album artists, a compilation or a
/// release type, sometimes no album or release date, one of a few sort keys for its artist and
/// album, and sometimes art, embedded or a cover, an artist image or a biography, each one of a
/// few.
fn random(rng: &mut Rng, root: &Path, path: PathBuf) -> TrackRecord {
    let image =
        |rng: &mut Rng| ImageInfo { hash: vec![(rng.next() % 4) as u8], width: 1, height: 1 };
    let names = |rng: &mut Rng, pool: &[&str], most: u64| -> Vec<String> {
        (0..rng.next() % (most + 1)).map(|_| rng.pick(pool).to_string()).collect()
    };
    let mut rec = TrackRecord { path, ..record(root, 0) };
    rec.tags.artists = names(rng, ARTISTS, 2);
    rec.tags.album_artists = if rng.chance(30) { names(rng, ARTISTS, 2) } else { Vec::new() };
    rec.tags.compilation = rng.chance(15);
    rec.tags.album = rng.chance(80).then(|| rng.pick(ALBUMS).to_string());
    rec.tags.set_values("genre", names(rng, GENRES, 3));
    rec.tags.set_values("mood", names(rng, GENRES, 2));
    rec.tags.labels = names(rng, LABELS, 2);
    rec.tags.disc_number = *rng.pick(&[None, Some(1), Some(2)]);
    rec.tags.track_number = *rng.pick(&[None, Some(1), Some(2)]);
    rec.artwork = rng.chance(60).then(|| Artwork {
        source: if rng.chance(50) {
            ArtworkSource::Embedded
        } else {
            ArtworkSource::Sidecar(root.join("cover.png"))
        },
        info: image(rng),
    });
    rec.artist_image =
        rng.chance(40).then(|| SidecarImage { path: root.join("artist.png"), info: image(rng) });
    rec.artist_biography =
        rng.chance(40).then(|| Biography::new(rng.pick(&["One.", "Two.", "Three."])));
    rec.tags.disc_total = *rng.pick(&[None, Some(2)]);
    rec.tags.track_total = *rng.pick(&[None, Some(3), Some(12)]);
    rec.tags.release_date =
        rng.chance(70).then(|| PartialDate::parse(rng.pick(&["1999", "2001-05"]))).flatten();
    rec.tags.release_type = rng.chance(40).then(|| rng.pick(TYPES).to_string());
    rec.sort.artist = rng.pick(SORTS).to_string();
    rec.sort.album_artist = rng.pick(SORTS).to_string();
    rec.sort.album = rng.pick(SORTS).to_string();
    rec.properties.duration_ms = 1000 * (1 + rng.next() % 300);
    rec
}

/// Every track by its file, with its ID, library, and whether it is missing.
fn tracks(store: &SqliteStore) -> BTreeMap<PathBuf, (TrackId, i64, bool)> {
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
    rows.into_iter().map(|(root, rel, id, lib, gone)| (file(root, rel), (id, lib, gone))).collect()
}

#[test]
fn derived_values_equal_a_recompute_after_every_change() {
    let (tmp, db) = open();
    let [a, b, c]: [PathBuf; 3] = ["a", "b", "c"].map(|r| tmp.path().join(r));
    let roots = (a.clone(), b.clone(), c.clone());
    db.write_blocking(move |tx| {
        libraries::create(tx, Some(1), "Two roots", &[&roots.0, &roots.1], &[])?;
        libraries::create(tx, Some(2), "Other", &[&roots.2], &[]).map(drop)
    })
    .unwrap();
    let store = SqliteStore::new(db).unwrap();
    let (one, two): (LibraryId, LibraryId) = (1, 2);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut ran = [0; 10];

    for scan in 1..=400 {
        let before = snapshot(&store);
        let known = tracks(&store);
        // Library 1's tracks, present or missing, in path order so the run repeats exactly.
        let of_one = |gone: bool| -> Vec<(PathBuf, TrackId)> {
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
                    let n = rng.next() % 12;
                    let path = root.join(format!("{}/{n}.flac", n % 3));
                    if upserts.iter().all(|u| u.path != path) {
                        upserts.push(TrackRecord { id: id(&path), ..random(&mut rng, root, path) });
                    }
                }
                let returned =
                    missing.first().filter(|(p, _)| upserts.iter().all(|u| &u.path != p));
                let returned = returned.map(|(_, id)| *id).into_iter().collect();
                store.apply(one, Batch { upserts, returned, ..Batch::default() }).unwrap();
            }
            // A file moved to the other root, retagged on the way.
            4 if !present.is_empty() => {
                let (from, id) = rng.pick(&present).clone();
                let to = if from.starts_with(&a) { &b } else { &a };
                let path = to.join(format!("moved {scan}.flac"));
                let rec = TrackRecord { id: Some(id), ..random(&mut rng, to, path) };
                store.apply(one, Batch { upserts: vec![rec], ..Batch::default() }).unwrap();
            }
            // A scan of one root whose listings lack one of its folders, or about half of its
            // files, which go missing.
            5 => {
                let root = rng.pick(&[&a, &b]).as_path();
                let on_root = present.iter().filter(|(p, _)| p.starts_with(root));
                let missing = if rng.chance(50) {
                    let folder = root.join((rng.next() % 3).to_string());
                    on_root.filter(|(p, _)| p.starts_with(&folder)).map(|(_, id)| *id).collect()
                } else {
                    on_root.filter(|_| rng.chance(50)).map(|(_, id)| *id).collect()
                };
                store.apply(one, Batch { missing, ..Batch::default() }).unwrap();
            }
            // Missing files coming back unchanged.
            6 => {
                let returned: Vec<TrackId> = missing.iter().take(3).map(|(_, id)| *id).collect();
                store.apply(one, Batch { returned, ..Batch::default() }).unwrap();
            }
            // A missing file back with new tags.
            7 if !missing.is_empty() => {
                let (path, id) = rng.pick(&missing).clone();
                let root = if path.starts_with(&a) { &a } else { &b };
                let rec = TrackRecord { id: Some(id), ..random(&mut rng, root, path) };
                store.apply(one, Batch { upserts: vec![rec], ..Batch::default() }).unwrap();
            }
            // The other library, with the same names, which must count apart.
            8 => {
                let path = c.join(format!("{}.flac", rng.next() % 6));
                let rec = TrackRecord { id: id(&path), ..random(&mut rng, &c, path) };
                store.apply(two, Batch { upserts: vec![rec], ..Batch::default() }).unwrap();
            }
            // A track deleted outright, present or missing; outside tests only the purge of tracks
            // long missing deletes one.
            9 if !present.is_empty() => {
                let pool = if !missing.is_empty() && rng.chance(50) { &missing } else { &present };
                let id = rng.pick(pool).1;
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

/// A genre taken off a credited track on its own, which the scanner never does, as it takes a
/// track's artists off first: the artist's count of the genre follows all the same. Another track
/// keeps the genre, so the genre, and with it the count, is not deleted outright.
#[test]
fn a_genre_taken_off_a_credited_track_is_taken_off_its_artist() {
    let (tmp, db) = open();
    let root = tmp.path().join("a");
    let created = root.clone();
    db.write_blocking(move |tx| {
        libraries::create(tx, Some(1), "Music", &[&created], &[]).map(drop)
    })
    .unwrap();
    let store = SqliteStore::new(db).unwrap();
    let tagged = |i, genres: &[&str]| {
        let mut rec = record(&root, i);
        rec.tags.set_values("genre", genres.iter().map(|genre| genre.to_string()).collect());
        rec
    };
    let upserts = vec![tagged(0, &["Rock", "Jazz"]), tagged(1, &["Rock"])];
    store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
    check(&store, "two tracks by one artist");

    store
        .db()
        .write_blocking(|tx| {
            let (track, tag): (i64, i64) = tx.query_row(
                "SELECT track_id, tag_id FROM track_tags WHERE tag_name = 'Rock' \
                 AND track_id IN (SELECT track_id FROM track_tags WHERE tag_name = 'Jazz')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            tx.execute("DELETE FROM track_tags WHERE track_id = ?1 AND tag_id = ?2", [track, tag])?;
            let touched = Touched { tags: HashSet::from([tag]), ..Touched::default() };
            catalog::recompute(tx, 1, &touched, 0)
        })
        .unwrap();
    check(&store, "taking a genre off one of them");
}

/// A name carried in two tag types is two tags, each brought to the album and artist on its own,
/// and retagging a file out of one type leaves the other (`design/database.md` §3).
#[test]
fn a_name_in_two_tag_types_is_two_tags() {
    let (tmp, db) = open();
    let root = tmp.path().join("a");
    let created = root.clone();
    db.write_blocking(move |tx| {
        libraries::create(tx, Some(1), "Music", &[&created], &[]).map(drop)
    })
    .unwrap();
    let store = SqliteStore::new(db).unwrap();
    let tags = |store: &SqliteStore| -> Vec<(String, String, i64, i64, i64)> {
        store
            .db()
            .read_blocking(|conn| {
                conn.prepare(
                    "SELECT type, name, track_count, \
                     (SELECT COUNT(*) FROM album_tags WHERE tag_id = tags.id), \
                     (SELECT COUNT(*) FROM artist_tags WHERE tag_id = tags.id) \
                     FROM tags ORDER BY type",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                .collect()
            })
            .unwrap()
    };
    // Each tag is on one track, album and artist, and is named as its own type spells it.
    let tag = |key: &str, name: &str| (key.to_owned(), name.to_owned(), 1, 1, 1);

    let mut rec = record(&root, 0);
    rec.tags.set_values("genre", vec!["Ambient".into()]);
    rec.tags.set_values("mood", vec!["ambient".into()]);
    store.apply(1, Batch { upserts: vec![rec.clone()], ..Batch::default() }).unwrap();
    assert_eq!(tags(&store), [tag("genre", "Ambient"), tag("mood", "ambient")]);
    check(&store, "one name in two types");

    let id = tracks(&store).get(&rec.path).map(|t| t.0);
    rec.tags.set_values("mood", Vec::new());
    store.apply(1, Batch { upserts: vec![TrackRecord { id, ..rec }], ..Batch::default() }).unwrap();
    assert_eq!(tags(&store), [tag("genre", "Ambient")]);
    check(&store, "retagged out of one type");
}

/// Tags read again from an unchanged file replace its tags of every type, at the current
/// generation, and keep the rest of the track, its analysis included; the derived values follow
/// (`design/scanning.md` §2).
#[test]
fn tags_read_again_keep_the_analysis() {
    let (tmp, db) = open();
    let root = tmp.path().join("a");
    let created = root.clone();
    db.write_blocking(move |tx| {
        libraries::create(tx, Some(1), "Music", &[&created], &[]).map(drop)
    })
    .unwrap();
    let store = SqliteStore::new(db).unwrap();
    let mut rec = record(&root, 0);
    rec.tags.set_values("genre", vec!["Rock".into()]);
    store.apply(1, Batch { upserts: vec![rec.clone()], ..Batch::default() }).unwrap();
    let id = tracks(&store)[&rec.path].0;
    store
        .db()
        .write_blocking(move |tx| {
            tx.execute(
                "UPDATE tracks SET analyzer_version = 3, loudness_lufs = -9.0, tags_generation = 0 \
                 WHERE id = ?1",
                [id],
            )
        })
        .unwrap();

    let tag_values = BTreeMap::from([
        ("genre".to_owned(), vec!["Jazz".to_owned()]),
        ("mood".to_owned(), vec!["Red".to_owned()]),
    ]);
    let rereads = vec![TagsReread { track_id: id, tag_values, tags_generation: 7 }];
    store.apply(1, Batch { rereads, ..Batch::default() }).unwrap();
    check(&store, "tags read again");
    let (analyzed, generation, tags): (i64, u32, String) = store
        .db()
        .read_blocking(move |conn| {
            conn.query_row(
                "SELECT analyzer_version, tags_generation, \
                 (SELECT group_concat(type || ':' || name, ' ') FROM \
                  (SELECT type, name FROM tags ORDER BY type)) \
                 FROM tracks WHERE id = ?1 AND loudness_lufs = -9.0",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
        })
        .unwrap();
    assert_eq!((analyzed, generation), (3, 7), "it records the generation they were read with");
    assert_eq!(tags, "genre:Jazz mood:Red", "Rock goes with its last track");
}
