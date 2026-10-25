//! The generated library the benchmarks run on, at a large library's scale
//! (`requirements/performance.md` §1, §8), and how they report what they measure.

use std::path::Path;
use std::time::{Duration, Instant};

use jewelcase_core::{Format, sort_key};
use jewelcase_scanner::testing::stamp_without_sidecars;
use jewelcase_scanner::*;

use crate::db::OPTIMIZE;
use crate::db::store::SqliteStore;

/// How many tracks a benchmark generates: `BENCH_TRACKS`, or 500,000 when it is unset. Panics
/// when it is set to anything but a number of tracks, rather than run at another size.
pub fn size() -> usize {
    std::env::var("BENCH_TRACKS")
        .map_or(500_000, |tracks| tracks.parse().expect("BENCH_TRACKS is a number of tracks"))
}

/// SplitMix64's finalizer: a well-mixed number from `n`, so each choice the generator makes is a
/// function of where it is made and the library is the same on every run.
fn mix(n: u64) -> u64 {
    let mut z = n.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `tracks` records under `root`, as a cold scan reads them, in path order. Artists are named
/// by number, and each has up to `tracks / 400` albums, `tracks / 400 / (1 + r)` for a random `r`
/// below `tracks / 14` but at least one, of 6 to 16 tracks: so a few artists, wherever they fall,
/// hold thousands of tracks, and most hold one album. An eighth of the tracks also credit one of
/// the first 200 artists. Each album has one of 40 genres, the first few far more common. The
/// last fiftieth are untagged files, 25 to a folder, all on the unknown album. The store draws
/// each track's ID at random, so ID order is neither path nor sort order.
pub fn records(root: &Path, tracks: usize) -> impl Iterator<Item = TrackRecord> {
    let untagged = tracks / 50;
    let (top, spread) = ((tracks / 400).max(1) as u64, (tracks / 14).max(1) as u64);
    let stamp = stamp_without_sidecars(root);
    let tagged = (0..)
        .flat_map(move |artist: u64| {
            let albums = (top / (1 + mix(artist) % spread)).max(1);
            (0..albums).map(move |n| (artist, n))
        })
        .flat_map(move |(artist, n)| {
            let album = artist << 20 | n;
            (1..=6 + mix(album) % 11).map(move |number| (artist, album, number))
        })
        .take(tracks - untagged)
        .map(move |(artist, album, number)| tagged(root, artist, album, number, stamp));
    let untagged = (0..untagged as u64).map(move |n| {
        let path = root.join(format!("Unsorted/{:05}/{n:06}.flac", n / 25));
        let unknown = sort_key("", None);
        let title = sort_key(&format!("{n:06}"), None);
        let sort = SortKeys { title, artist: unknown.clone(), album: unknown };
        record(root, &path, n << 8, TagSet::default(), sort, stamp)
    });
    tagged.chain(untagged)
}

/// The record of track `number` on `album` by `artist`. `album` holds `artist` above its low 20
/// bits and the album's place among that artist's albums in them.
fn tagged(root: &Path, artist: u64, album: u64, number: u64, stamp: SidecarStamp) -> TrackRecord {
    let seed = album << 8 | number;
    let name = |artist: u64| format!("Artist {artist:06}");
    let (artist_name, title) = (name(artist), format!("Track {:08x}", mix(seed) & 0xFFFF_FFFF));
    let album_title = format!("Album {:05} {:08x}", album & 0xF_FFFF, mix(album ^ 8) & 0xFFFF_FFFF);
    let path = root.join(format!("{artist_name}/{album_title}/{number:02} - {title}.flac"));
    let mut artists = vec![artist_name.clone()];
    let featured = mix(seed ^ 2) % 200;
    if mix(seed ^ 1).is_multiple_of(8) && featured != artist {
        artists.push(name(featured));
    }
    let genre = (mix(album ^ 3) % 1000).pow(3) * 40 / 1_000_000_000;
    let sort = SortKeys {
        title: sort_key(&title, None),
        artist: sort_key(&artist_name, None),
        album: sort_key(&album_title, None),
    };
    let tags = TagSet {
        title: Some(title),
        artists,
        album: Some(album_title),
        album_artists: vec![artist_name],
        track_number: u32::try_from(number).ok(),
        genres: vec![format!("Genre {genre}")],
        release_date: PartialDate::parse(&(1960 + mix(album ^ 4) % 65).to_string()),
        ..TagSet::default()
    };
    record(root, &path, seed, tags, sort, stamp)
}

/// The record of a FLAC at `path` holding `tags`, its size and mtime drawn from `seed`.
fn record(
    root: &Path,
    path: &Path,
    seed: u64,
    tags: TagSet,
    sort: SortKeys,
    stamp: SidecarStamp,
) -> TrackRecord {
    TrackRecord {
        id: None,
        root: root.to_path_buf(),
        path: path.to_path_buf(),
        size: 3_000_000 + mix(seed ^ 5) % 60_000_000,
        mtime_ms: 1_600_000_000_000 + mix(seed ^ 6) % 100_000_000_000,
        tags,
        properties: AudioProperties {
            format: Format::Flac,
            duration_ms: 60_000 + mix(seed ^ 7) % 540_000,
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate_kbps: Some(900),
        },
        artwork: None,
        artist_image: None,
        artist_biography: None,
        lyrics_sidecar: None,
        sidecar_stamp: stamp,
        sort,
    }
}

/// Applies `records` to library 1 of `store` in batches of a scan's size, as a cold scan
/// does, and returns how long each batch held the writer. Then gathers planner statistics, as
/// the server does when it opens the database.
pub fn fill(store: &SqliteStore, records: impl Iterator<Item = TrackRecord>) -> Vec<Duration> {
    let batch = ScanOptions::default().batch_size;
    let mut records = records.peekable();
    let mut times = Vec::new();
    while records.peek().is_some() {
        let upserts = records.by_ref().take(batch).collect();
        let started = Instant::now();
        store.apply(1, Batch { upserts, ..Batch::default() }).unwrap();
        times.push(started.elapsed());
    }
    store.db().write_blocking(|tx| tx.execute_batch(OPTIMIZE)).unwrap();
    times
}

/// `times`' median, 95th and 99th percentiles and maximum (`requirements/performance.md` §8),
/// as one line. Sorts `times`, which must not be empty.
pub fn summary(times: &mut [Duration]) -> String {
    times.sort();
    let at = |percent: usize| times[(times.len() * percent / 100).min(times.len() - 1)];
    let (p50, p95, p99, max) = (at(50), at(95), at(99), times[times.len() - 1]);
    format!("p50 {p50:>9.2?}  p95 {p95:>9.2?}  p99 {p99:>9.2?}  max {max:>9.2?}")
}

/// The generated files come in path order, as a scan walks them, each path once.
#[test]
fn records_come_in_path_order() {
    let paths: Vec<_> = records(Path::new("/music"), 20_000).map(|rec| rec.path).collect();
    assert_eq!(paths.len(), 20_000);
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
}
