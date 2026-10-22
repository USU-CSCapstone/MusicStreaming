//! The store's share of scanning's cost (`requirements/performance.md` §5), measured on a
//! large generated library, on request.

use std::fs::{self, File};
use std::time::{Duration, Instant, UNIX_EPOCH};

use jewelcase_scanner::testing;
use jewelcase_scanner::*;

use super::{make, scalar};
use crate::db::testing::scale::{self, summary};

/// Makes `rec`'s file as a scan would find it unchanged: sparse, so a library of them takes next
/// to no space, with the record's size and mtime, so the scan never reads it.
fn sparse(rec: &TrackRecord) {
    fs::create_dir_all(rec.path.parent().unwrap()).unwrap();
    let file = File::create(&rec.path).unwrap();
    file.set_len(rec.size).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_millis(rec.mtime_ms)).unwrap();
}

/// Rewrites the file of each of `recs` as a small WAV tagged with its record's title, first
/// artist and album, when it has them, so a scan finds it changed and reads it.
fn change(recs: &[TrackRecord]) {
    for rec in recs {
        testing::write_small_wav(&rec.path);
        let tags = &rec.tags;
        if let (Some(title), [artist, ..], Some(album)) =
            (&tags.title, &tags.artists[..], &tags.album)
        {
            testing::tag_wav(&rec.path, title, artist, album);
        }
    }
}

/// How skewed the generated library is: its artists, albums and genres, and the most tracks one
/// of each holds.
const SHAPE: &str = "SELECT (SELECT COUNT(*) FROM tracks) || ' tracks; ' \
     || (SELECT COUNT(*) || ' artists, up to ' || MAX(track_count) FROM artists) || ' tracks; ' \
     || (SELECT COUNT(*) || ' albums, up to ' || MAX(track_count) FROM albums) || ' tracks; ' \
     || (SELECT COUNT(*) || ' genres, up to ' || MAX(track_count) FROM tags) || ' tracks'";

/// The scanning benchmarks, on a generated library ([`scale::records`]) whose files are on disk:
/// the batches of a cold scan, store side, as the library grows; an unchanged rescan, right after
/// the files were written, so with the folders' listings and the database in the page cache; a
/// ten-file incremental scan; and the analysis queue's pick. The library and its database go in
/// the temporary folder, so run it with that on an SSD whose filesystem keeps sparse files (ext4,
/// btrfs or XFS), not a tmpfs:
/// `TMPDIR=<folder> cargo test --release -p jewelcase-server benchmark_scanning -- --ignored --nocapture`.
/// `BUDGET_TRACKS` sets the library size (default 500,000).
#[test]
#[ignore = "benchmark"]
fn benchmark_scanning_budgets() {
    let tracks = scale::size();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);

    // Ten files spread evenly through the library, each in a folder of its own, the last on the
    // untagged album.
    let picks: Vec<usize> = (0..10).map(|k| (tracks - 1) * k / 9).collect();
    let mut changing = Vec::new();
    let generated = scale::records(&root, tracks).enumerate().inspect(|(i, rec)| {
        sparse(rec);
        if picks.contains(i) {
            changing.push(rec.clone());
        }
    });
    let mut batches = scale::fill(&store, generated.map(|(_, rec)| rec));
    let total: Duration = batches.iter().sum();
    let tenth = batches.len().div_ceil(10);
    let last = batches.len() - tenth;
    eprintln!("{}", scalar::<String>(&store, SHAPE).unwrap());
    eprintln!("cold scan's store side: {total:.1?} (budget 1 h with reading the files)");
    eprintln!("  first tenth's batches: {}", summary(&mut batches[..tenth], None));
    eprintln!("  last tenth's batches:  {}", summary(&mut batches[last..], None));

    let started = Instant::now();
    let progress = testing::scan_library(&store, &lib).progress;
    let mut rescan = [started.elapsed()];
    assert_eq!(progress.files_seen, tracks as i64);
    assert_eq!((progress.added, progress.updated, progress.missing), (0, 0, 0));
    eprintln!("unchanged rescan: {}", summary(&mut rescan, None));

    let folders = changing
        .iter()
        .map(|rec| Scope::folder(&root, rec.path.parent().unwrap(), Depth::Directory));
    let folders: Vec<Scope> = folders.collect();
    let mut times = Vec::new();
    for _ in 0..20 {
        change(&changing);
        let started = Instant::now();
        let scan = testing::scan_once(&store, &lib, Trigger::Watch, folders.clone());
        times.push(started.elapsed());
        assert_eq!(scan.progress.updated, 10);
    }
    let budget = Some(Duration::from_secs(1));
    eprintln!("ten-file incremental scan: {}", summary(&mut times, budget));

    let pick = |state: &str| {
        let mut times: Vec<Duration> = (0..100)
            .map(|_| {
                let started = Instant::now();
                let picked = store.next_unanalyzed(lib.id, ANALYZER_VERSION, 1).unwrap();
                assert_eq!(picked.len(), 1);
                started.elapsed()
            })
            .collect();
        eprintln!("next_unanalyzed, {state}: {}", summary(&mut times, None));
    };
    pick("none analyzed");
    let analyze = "UPDATE tracks SET analyzer_version = ?1 WHERE id % 100 <> 0";
    store.db().write_blocking(|tx| tx.execute(analyze, [ANALYZER_VERSION])).unwrap();
    pick("one in a hundred left");
}
