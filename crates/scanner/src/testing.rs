//! Fixtures and the store conformance suite (`design/scanning.md` §15).
//!
//! Every [`Store`] implementation runs the same scenarios through the real
//! pipeline. The in-memory store and the server's SQLite store both run them
//! through [`store_suite!`] and must pass unchanged. The move scenarios are
//! ignored until track identity across moves is decided (`design/general.md` §11).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use jewelcase_core::Format;
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag, TagType};

use crate::governor::Governor;
use crate::queue::Scanner;
use crate::scan::{ScanContext, ScanOptions, run_scan};
use crate::store::{Batch, FeedEntry, MemoryStore, Store};
use crate::types::*;

mod jobs;
mod reanalysis;
mod shutdown;
mod unread;
pub use jobs::*;
pub use reanalysis::*;
pub use shutdown::*;
pub use unread::*;

// ───────────────────────────── Inspection ─────────────────────────────

/// A store-agnostic view of one indexed track, for assertions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectTrack {
    pub id: TrackId,
    pub path: PathBuf,
    pub missing: bool,
    /// The displayed title: the tag, or the filename stem.
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub has_artwork: bool,
    pub has_biography: bool,
    pub has_sidecar_lyrics: bool,
    pub sort_title: String,
    pub sort_artist: String,
    pub format: Format,
    pub channels: Option<u8>,
    pub sample_rate: Option<u32>,
    pub analyzed: bool,
    /// Whether its analysis measured a loudness.
    pub measured: bool,
}

/// What a store must expose for the suite to check it.
pub trait InspectStore: Store {
    /// Every track of `library`, missing or not.
    fn inspect_tracks(&self, library: LibraryId) -> Vec<InspectTrack>;
    /// Every problem recorded for `library`.
    fn inspect_problems(&self, library: LibraryId) -> Vec<Problem>;
    /// Every scan of `library`.
    fn inspect_scans(&self, library: LibraryId) -> Vec<Scan>;
    /// The change feed's head: the position of its latest entry, or zero when it is
    /// empty. Every entry written raises it, so an unchanged head means nothing was
    /// written, even to a feed that keeps one entry per item.
    fn feed_head(&self, library: LibraryId) -> i64;
    /// The tracks named by feed entries after position `head`, oldest first.
    fn feed_tracks_after(&self, library: LibraryId, head: i64) -> Vec<TrackId>;
    /// The placeholder stored for the image with content `hash`.
    fn inspect_placeholder(&self, library: LibraryId, hash: &[u8]) -> Option<Vec<u8>>;
}

impl InspectStore for MemoryStore {
    fn inspect_tracks(&self, library: LibraryId) -> Vec<InspectTrack> {
        self.tracks(library)
            .into_iter()
            .map(|t| InspectTrack {
                id: t.record.id.unwrap_or(0),
                path: t.record.path.clone(),
                missing: t.missing,
                title: t.record.display_title(),
                artists: t.record.tags.artists.clone(),
                album: t.record.tags.album.clone(),
                has_artwork: t.record.artwork.is_some(),
                has_biography: t.record.artist_biography.is_some(),
                has_sidecar_lyrics: t.record.lyrics_sidecar.is_some(),
                sort_title: t.record.sort.title.clone(),
                sort_artist: t.record.sort.artist.clone(),
                format: t.record.properties.format,
                channels: t.record.properties.channels,
                sample_rate: t.record.properties.sample_rate,
                analyzed: t.analysis.is_some(),
                measured: t.analysis.is_some_and(|a| a.integrated_lufs.is_some()),
            })
            .collect()
    }

    fn inspect_placeholder(&self, library: LibraryId, hash: &[u8]) -> Option<Vec<u8>> {
        self.placeholder(library, hash)
    }

    fn inspect_problems(&self, library: LibraryId) -> Vec<Problem> {
        self.problems(library)
    }

    fn inspect_scans(&self, library: LibraryId) -> Vec<Scan> {
        self.scans(library)
    }

    fn feed_head(&self, library: LibraryId) -> i64 {
        // The memory feed only appends, so its length is its head.
        i64::try_from(self.feed(library).len()).unwrap()
    }

    fn feed_tracks_after(&self, library: LibraryId, head: i64) -> Vec<TrackId> {
        self.feed(library)
            .into_iter()
            .skip(usize::try_from(head).unwrap())
            .map(|entry| match entry {
                FeedEntry::Added(id)
                | FeedEntry::Updated(id)
                | FeedEntry::Missing(id)
                | FeedEntry::Returned(id) => id,
            })
            .collect()
    }
}

// ───────────────────────────── Fixtures ─────────────────────────────

/// Write a stereo 16-bit WAV of a sine at `hz` and `amplitude` lasting
/// `seconds`.
pub fn write_sine_wav(path: &Path, hz: f32, amplitude: f32, seconds: f32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let n = (seconds * spec.sample_rate as f32) as u32;
    for i in 0..n {
        let t = i as f32 / spec.sample_rate as f32;
        let v = (amplitude * (2.0 * std::f32::consts::PI * hz * t).sin() * i16::MAX as f32) as i16;
        w.write_sample(v).unwrap();
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}

/// A tiny valid WAV: 0.2 s of quiet tone.
pub fn write_small_wav(path: &Path) {
    write_sine_wav(path, 440.0, 0.1, 0.2);
}

/// A 1x1 PNG, for sidecar art.
pub const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

pub fn tag_wav(path: &Path, title: &str, artist: &str, album: &str) {
    let mut tag = Tag::new(TagType::Id3v2);
    tag.insert_text(ItemKey::TrackTitle, title.into());
    tag.insert_text(ItemKey::TrackArtist, artist.into());
    tag.insert_text(ItemKey::AlbumTitle, album.into());
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

/// Tag a WAV with arbitrary text items.
pub fn tag_wav_with(path: &Path, items: &[(ItemKey, &str)]) {
    let mut tag = Tag::new(TagType::Id3v2);
    for (key, value) in items {
        tag.push(lofty::tag::TagItem::new(*key, lofty::tag::ItemValue::Text((*value).to_owned())));
    }
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

pub fn make_album(root: &Path, artist: &str, album: &str, n: usize) {
    for i in 1..=n {
        let p = root.join(artist).join(album).join(format!("{i:02} - Track {i}.wav"));
        write_small_wav(&p);
        tag_wav(&p, &format!("Track {i}"), artist, album);
    }
}

/// Library 1, with `root` as its one root and no excludes.
pub fn library(root: &Path) -> LibraryConfig {
    LibraryConfig { id: 1, roots: vec![root.to_path_buf()], excludes: Vec::new() }
}

/// Run one scan of `scopes` synchronously against `store`.
pub fn scan_once(
    store: &dyn Store,
    lib: &LibraryConfig,
    trigger: Trigger,
    scopes: Vec<Scope>,
) -> Scan {
    let governor = Governor::new();
    let cancel = AtomicBool::new(false);
    let options = ScanOptions::default();
    let mut scan = Scan {
        id: 0,
        library: lib.id,
        trigger,
        scopes,
        state: ScanState::Queued,
        started_at: None,
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: None,
    };
    scan.id = store.create_scan(&scan).unwrap();
    let ctx = ScanContext {
        store,
        library: lib,
        governor: &governor,
        cancel: &cancel,
        shutdown: &AtomicBool::new(false),
        options: &options,
    };
    run_scan(&ctx, &mut scan);
    scan
}

pub fn scan_library(store: &dyn Store, lib: &LibraryConfig) -> Scan {
    scan_once(store, lib, Trigger::Manual, lib.roots.iter().map(Scope::root).collect())
}

/// The tracks of `lib` not marked missing, sorted by path.
fn live(store: &dyn InspectStore, lib: &LibraryConfig) -> Vec<InspectTrack> {
    let mut v: Vec<InspectTrack> =
        store.inspect_tracks(lib.id).into_iter().filter(|t| !t.missing).collect();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v
}

/// A store under test must be able to hold a library with the id
/// `library()` uses. Implementations that need a row for it (SQLite) do so
/// in the factory this suite is given.
pub type Factory<S> = fn(&Path) -> S;

// ───────────────────────────── Scenarios ─────────────────────────────

pub fn cold_scan_then_incremental_is_all_unchanged<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "Nirvana", "Nevermind", 3);
    make_album(&root, "Nirvana", "In Utero", 2);
    fs::write(root.join("Nirvana/artist.txt"), "bio").unwrap();
    fs::write(root.join("Nirvana/Nevermind/cover.png"), PNG).unwrap();
    fs::write(root.join("Nirvana/Nevermind/01 - Track 1.lrc"), "[00:01.00]hi").unwrap();
    fs::write(root.join("readme.txt"), "ignored").unwrap();
    fs::write(root.join("Nirvana/old.wma"), "ignored too").unwrap();

    let store = make(&root);
    let lib = library(&root);

    let first = scan_library(&store, &lib);
    assert_eq!(first.state, ScanState::Completed);
    assert_eq!(first.progress.files_seen, 5);
    assert_eq!(first.progress.added, 5);
    assert_eq!(first.progress.problems, 0, "{:?}", store.inspect_problems(lib.id));
    let tracks = live(&store, &lib);
    assert_eq!(tracks.len(), 5);

    let t1 = tracks.iter().find(|t| t.path.ends_with("Nevermind/01 - Track 1.wav")).unwrap();
    assert!(t1.has_artwork, "cover.png in the album folder");
    assert!(t1.has_biography, "artist.txt one level up");
    assert!(t1.has_sidecar_lyrics, "lyrics by stem");
    assert_eq!(t1.title, "Track 1");
    assert_eq!(t1.artists, vec!["Nirvana"]);
    assert_eq!(t1.sort_artist, "nirvana");
    let utero = tracks.iter().find(|t| t.path.ends_with("In Utero/01 - Track 1.wav")).unwrap();
    assert!(!utero.has_artwork, "no cover anywhere above In Utero");
    assert!(utero.has_biography, "biography inherited from the artist folder");

    let mut announced = store.feed_tracks_after(lib.id, 0);
    announced.sort_unstable();
    let mut ids: Vec<TrackId> = tracks.iter().map(|t| t.id).collect();
    ids.sort_unstable();
    assert_eq!(announced, ids, "one feed entry per track");
    let head_after_first = store.feed_head(lib.id);

    // Second scan: nothing opened, nothing changed.
    let second = scan_library(&store, &lib);
    assert_eq!(second.state, ScanState::Completed);
    assert_eq!(second.progress.files_seen, 5);
    assert_eq!(second.progress.files_processed, 0);
    assert_eq!(second.progress.added + second.progress.updated, 0);
    assert_eq!(second.progress.missing, 0);
    assert_eq!(
        store.feed_head(lib.id),
        head_after_first,
        "an unchanged scan must not touch the feed"
    );
}

pub fn retag_updates_in_place_and_keeps_identity<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    let before = live(&store, &lib)[0].clone();

    std::thread::sleep(Duration::from_millis(20));
    let p = root.join("A/X/01 - Track 1.wav");
    tag_wav(&p, "Renamed", "A", "X");
    fs::File::open(&p)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.updated, 1);
    let after = live(&store, &lib)[0].clone();
    assert_eq!(after.id, before.id);
    assert_eq!(after.title, "Renamed");
}

/// A lyrics file written beside a track whose audio did not change, as a plugin does,
/// is picked up by the next scan, and dropped when it goes (`requirements/scanning.md` §7).
pub fn lyrics_file_beside_an_unchanged_track<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    assert!(live(&store, &lib).iter().all(|t| !t.has_sidecar_lyrics));

    let lrc = root.join("A/X/01 - Track 1.lrc");
    fs::write(&lrc, "[00:01.00]hello\n").unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.updated, 1, "only the track beside the new file is re-read");
    let with: Vec<InspectTrack> =
        live(&store, &lib).into_iter().filter(|t| t.has_sidecar_lyrics).collect();
    assert_eq!(with.len(), 1);
    assert!(with[0].path.ends_with("01 - Track 1.wav"));

    let again = scan_library(&store, &lib);
    assert_eq!(again.progress.updated, 0, "and stays incremental afterwards");

    fs::remove_file(&lrc).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.updated, 1);
    assert!(live(&store, &lib).iter().all(|t| !t.has_sidecar_lyrics));
}

pub fn missing_then_returned<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);

    let p = root.join("A/X/02 - Track 2.wav");
    let bytes = fs::read(&p).unwrap();
    let meta = fs::metadata(&p).unwrap();
    let head_before_missing = store.feed_head(lib.id);
    fs::remove_file(&p).unwrap();

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.missing, 1);
    let missing: Vec<InspectTrack> =
        store.inspect_tracks(lib.id).into_iter().filter(|t| t.missing).collect();
    assert_eq!(missing.len(), 1);
    assert!(missing[0].path.ends_with("02 - Track 2.wav"));
    let id = missing[0].id;
    assert_eq!(
        store.feed_tracks_after(lib.id, head_before_missing),
        vec![id],
        "the loss is announced"
    );
    let head_before_return = store.feed_head(lib.id);

    // The file comes back byte-identical with its old mtime: reconnects.
    fs::write(&p, &bytes).unwrap();
    fs::File::open(&p).unwrap().set_modified(meta.modified().unwrap()).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.missing, 0);
    assert_eq!(scan.progress.added, 0);
    let t = store.inspect_tracks(lib.id).into_iter().find(|t| t.path == p).unwrap();
    assert!(!t.missing);
    assert_eq!(t.id, id);
    assert_eq!(
        store.feed_tracks_after(lib.id, head_before_return),
        vec![id],
        "the return is announced"
    );
    assert!(store.feed_head(lib.id) > head_before_return, "the head moves past the return");
}

pub fn unavailable_root_suspends_and_touches_nothing<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("nas");
    make_album(&root, "A", "X", 2);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    assert_eq!(live(&store, &lib).len(), 2);

    // "Unmount": the root vanishes.
    let stash = tmp.path().join("stash");
    fs::rename(&root, &stash).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Suspended);
    assert_eq!(scan.progress.missing, 0);
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));

    // An empty mount point is unavailable too.
    fs::create_dir(&root).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Suspended);
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));

    // Remount: everything unchanged, nothing re-added.
    fs::remove_dir(&root).unwrap();
    fs::rename(&stash, &root).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!(scan.progress.added, 0);
}

pub fn bad_file_is_a_problem_that_clears<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let bad = root.join("A/X/broken.flac");
    fs::write(&bad, b"this is not audio at all, just text pretending").unwrap();
    let store = make(&root);
    let lib = library(&root);

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Completed, "one bad file never aborts a scan");
    assert_eq!(scan.progress.added, 1);
    assert_eq!(scan.progress.problems, 1);
    let problems = store.inspect_problems(lib.id);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path, bad);
    assert_eq!(problems[0].kind, ProblemKind::CorruptAudio);

    // Replace with a real file: the problem clears.
    write_small_wav(&bad);
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.added, 1);
    assert!(store.inspect_problems(lib.id).is_empty());
}

pub fn a_symlink_is_a_problem<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let outside = tmp.path().join("outside.wav");
    write_small_wav(&outside);
    let link = root.join("A/X/linked.wav");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let store = make(&root);
    let lib = library(&root);

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.added, 1, "only the regular file");
    assert_eq!(scan.progress.problems, 1);
    let problems = store.inspect_problems(lib.id);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path, link);
    assert_eq!(problems[0].detail, "symlinks are not followed");
}

pub fn a_scope_through_a_folder_link_walks_nothing<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    // Another library's folder, linked into this one, as the watcher or a plugin may name it.
    make_album(&tmp.path().join("other"), "B", "Y", 1);
    let link = root.join("linked");
    std::os::unix::fs::symlink(tmp.path().join("other/B"), &link).unwrap();
    let store = make(&root);
    let lib = library(&root);

    let scope = Scope::folder(&root, &link, Depth::Subtree);
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.progress.files_seen, 0);
    assert_eq!(scan.progress.problems, 1);
    let problems = store.inspect_problems(lib.id);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path, link);
    assert!(live(&store, &lib).is_empty());
}

/// A scope on a folder that no longer exists marks its tracks missing, takes the problems under
/// it with them, and records none of its own.
pub fn a_scope_naming_a_deleted_folder_reconciles<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    make_album(&root, "B", "Y", 1);
    fs::write(root.join("A/X/broken.flac"), b"not audio").unwrap();
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    assert_eq!(store.inspect_problems(lib.id).len(), 1);
    fs::remove_dir_all(root.join("A/X")).unwrap();

    let scope = Scope::folder(&root, root.join("A/X"), Depth::Subtree);
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!(scan.progress.missing, 2);
    assert_eq!(scan.progress.problems, 0);
    assert_eq!(live(&store, &lib).len(), 1);
    assert_eq!(store.inspect_problems(lib.id), []);
}

/// A deleted file takes its problem with it, while a deleted file the scope does not reach, in a
/// sibling folder whose name extends the scope's or in a folder below it, keeps its problem
/// until its own folder is scanned.
pub fn a_deleted_file_takes_its_problem<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    for folder in ["A/X", "A/X 2", "A/X/Y"] {
        fs::create_dir_all(root.join(folder)).unwrap();
        fs::write(root.join(folder).join("broken.flac"), b"not audio").unwrap();
    }
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    assert_eq!(store.inspect_problems(lib.id).len(), 3);

    for folder in ["A/X", "A/X 2", "A/X/Y"] {
        fs::remove_file(root.join(folder).join("broken.flac")).unwrap();
    }
    let scope = Scope::folder(&root, root.join("A/X"), Depth::Directory);
    scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    let mut problems: Vec<PathBuf> =
        store.inspect_problems(lib.id).into_iter().map(|p| p.path).collect();
    problems.sort();
    assert_eq!(problems, [root.join("A/X/Y/broken.flac"), root.join("A/X 2/broken.flac")]);
    assert_eq!(live(&store, &lib).len(), 1);
}

/// A Subtree scope at a file, as the watcher makes when a folder is replaced by a file within
/// its wait, walks nothing: it records no problem and marks nothing missing, whether the file is
/// there or not.
pub fn a_scope_at_a_file_reconciles_nothing<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    let track = root.join("A/X/01 - Track 1.wav");
    let scope = Scope::folder(&root, &track, Depth::Subtree);

    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope.clone()]);
    assert_eq!(
        (scan.state, scan.progress.problems, scan.progress.missing),
        (ScanState::Completed, 0, 0)
    );
    assert_eq!(store.inspect_problems(lib.id), []);

    fs::remove_file(&track).unwrap();
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.progress.missing, 0);
    assert_eq!(live(&store, &lib).len(), 1);
}

/// What the watcher asks of a folder renamed away: whether a live track or a problem lies under
/// it, at any depth, not counting the folder's own path or a sibling whose name extends it.
pub fn indexes_under_sees_live_tracks_and_problems<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "A 2", "X", 1);
    fs::create_dir_all(root.join("B")).unwrap();
    fs::write(root.join("B/broken.flac"), b"not audio").unwrap();
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    let under = |path: &str| store.indexes_under(lib.id, &root.join(path)).unwrap();
    assert!(under("A") && under("A/X") && under("B"));
    assert!(!under("A/X/01 - Track 1.wav") && !under("B/broken.flac") && !under("C"));

    fs::remove_dir_all(root.join("A")).unwrap();
    scan_library(&store, &lib);
    assert!(!under("A"), "its tracks are missing");
    assert!(under("A 2"));
}

/// A symlink that leads nowhere keeps its problem: the link is there, though its target is not.
pub fn a_dangling_link_keeps_its_problem<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let link = root.join("A/X/dangling.flac");
    std::os::unix::fs::symlink(root.join("A/X/nowhere.flac"), &link).unwrap();
    let store = make(&root);
    let lib = library(&root);

    scan_library(&store, &lib);
    let problems: Vec<PathBuf> =
        store.inspect_problems(lib.id).into_iter().map(|p| p.path).collect();
    assert_eq!(problems, [link]);
}

/// A problem the walk cannot see the cause of, such as one recorded against a track by its
/// analysis, stays while the track's file is there.
pub fn a_problem_on_a_present_file_stays<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    let track = root.join("A/X/01 - Track 1.wav");
    let mut batch = Batch::new(0);
    batch.problems.push(Problem {
        path: track.clone(),
        kind: ProblemKind::CorruptAudio,
        detail: "decode failed".into(),
        seen_at: std::time::SystemTime::UNIX_EPOCH,
    });
    store.apply(lib.id, batch).unwrap();

    scan_library(&store, &lib);
    let problems: Vec<PathBuf> =
        store.inspect_problems(lib.id).into_iter().map(|p| p.path).collect();
    assert_eq!(problems, [track]);
}

/// A folder that could not be listed loses its problem once it lists. Skipped, saying so, where
/// permissions do not stop this user reading, as for root.
pub fn a_folder_problem_clears_once_it_lists<S: InspectStore>(make: Factory<S>) {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let store = make(&root);
    let lib = library(&root);
    let folder = root.join("A/X");
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&folder).is_ok() {
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("skipped: permissions do not stop this user reading a folder (running as root?)");
        return;
    }
    scan_library(&store, &lib);
    fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
    let problems: Vec<PathBuf> =
        store.inspect_problems(lib.id).into_iter().map(|p| p.path).collect();
    assert_eq!(problems, [folder]);

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.added, 1);
    assert_eq!(store.inspect_problems(lib.id), []);
}

/// A folder that cannot be listed, and one whose entries cannot be read, keep their tracks
/// and record problems, while reconcile still runs everywhere else (`requirements/scanning.md`
/// §7). Skipped, saying so, where permissions do not stop this user reading, as for root.
pub fn an_unreadable_folder_keeps_its_tracks<S: InspectStore>(make: Factory<S>) {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 2);
    // A sibling whose name extends B/Y's, which a text prefix would wrongly match.
    make_album(&root, "B", "Yz", 2);
    make_album(&root, "C", "Z", 2);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    fs::remove_file(root.join("A/X/01 - Track 1.wav")).unwrap();
    fs::remove_file(root.join("B/Yz/01 - Track 1.wav")).unwrap();

    let set_mode = |folder: &str, mode| {
        fs::set_permissions(root.join(folder), fs::Permissions::from_mode(mode)).unwrap()
    };
    // B/Y cannot be listed; C/Z lists its names, but none of them can be read.
    set_mode("B/Y", 0o000);
    set_mode("C/Z", 0o400);
    if fs::read_dir(root.join("B/Y")).is_ok() {
        set_mode("B/Y", 0o755);
        set_mode("C/Z", 0o755);
        eprintln!("skipped: permissions do not stop this user reading a folder (running as root?)");
        return;
    }
    let scan = scan_library(&store, &lib);
    set_mode("B/Y", 0o755);
    set_mode("C/Z", 0o755);

    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!(scan.progress.missing, 2, "only the deleted files");
    assert_eq!(live(&store, &lib).len(), 5);
    let mut problems: Vec<(PathBuf, ProblemKind)> =
        store.inspect_problems(lib.id).into_iter().map(|p| (p.path, p.kind)).collect();
    problems.sort_by(|a, b| a.0.cmp(&b.0));
    let denied = |path: &str| (root.join(path), ProblemKind::PermissionDenied);
    assert_eq!(
        problems,
        [denied("B/Y"), denied("C/Z/01 - Track 1.wav"), denied("C/Z/02 - Track 2.wav")]
    );
}

pub fn untagged_file_is_named_after_itself<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let p = root.join("loose/My Song.wav");
    write_small_wav(&p);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    let t = live(&store, &lib)[0].clone();
    assert_eq!(t.title, "My Song", "named after its file");
    assert!(t.artists.is_empty());
    assert_eq!(t.sort_title, "my song");
    assert_eq!(t.format, Format::Wav);
    assert_eq!(t.channels, Some(2));
    assert_eq!(t.sample_rate, Some(44_100));
}

pub fn excludes_are_never_indexed<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "backup", "X", 1);
    let store = make(&root);
    let mut lib = library(&root);
    lib.excludes = vec!["backup".into(), "backup/**".into()];
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.files_seen, 1);
    assert_eq!(live(&store, &lib).len(), 1);
}

pub fn folder_scope_reconciles_only_inside_itself<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);

    fs::remove_file(root.join("B/Y/01 - Track 1.wav")).unwrap();
    let scan = scan_once(
        &store,
        &lib,
        Trigger::Manual,
        vec![Scope::folder(&root, root.join("A"), Depth::Subtree)],
    );
    assert_eq!(scan.progress.missing, 0);
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));
    let scan = scan_once(
        &store,
        &lib,
        Trigger::Manual,
        vec![Scope::folder(&root, root.join("B"), Depth::Subtree)],
    );
    assert_eq!(scan.progress.missing, 1);
}

/// A Subtree scope on a non-ASCII folder, which is longer in bytes than in characters,
/// reconciles only inside that folder. `Björk 1 (Live)` sorts just before `Björk 1/` and
/// `Björk 10` just after it, so neither range bound can move by one character unnoticed.
pub fn non_ascii_subtree_reconciles_only_inside_itself<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "Björk 1", "Début", 2);
    make_album(&root, "Björk 1 (Live)", "X", 1);
    make_album(&root, "Björk 10", "X", 1);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);

    for path in ["Björk 1/Début", "Björk 1 (Live)/X", "Björk 10/X"] {
        fs::remove_file(root.join(path).join("01 - Track 1.wav")).unwrap();
    }
    let scope = Scope::folder(&root, root.join("Björk 1"), Depth::Subtree);
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.progress.missing, 1);
    let live: Vec<PathBuf> = live(&store, &lib).into_iter().map(|t| t.path).collect();
    assert_eq!(
        live,
        [
            root.join("Björk 1/Début/02 - Track 2.wav"),
            root.join("Björk 1 (Live)/X/01 - Track 1.wav"),
            root.join("Björk 10/X/01 - Track 1.wav"),
        ]
    );
}

/// A Directory scope under a non-ASCII folder reconciles its own files and none in the folder
/// below it, whose one-character name the folder's extra bytes would otherwise skip past.
pub fn non_ascii_directory_reconciles_only_itself<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "Björk", "Début", 2);
    make_album(&root, "Björk/Début", "1", 1);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);

    fs::remove_file(root.join("Björk/Début/01 - Track 1.wav")).unwrap();
    let scope = Scope::folder(&root, root.join("Björk/Début"), Depth::Directory);
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.progress.missing, 1);
    let live: Vec<PathBuf> = live(&store, &lib).into_iter().map(|t| t.path).collect();
    assert_eq!(
        live,
        [root.join("Björk/Début/02 - Track 2.wav"), root.join("Björk/Début/1/01 - Track 1.wav")]
    );
}

/// A Directory scope on the root reconciles the root's own files and none in its folders.
pub fn root_directory_reconciles_only_its_own_files<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    write_small_wav(&root.join("loose.wav"));
    make_album(&root, "A", "X", 1);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);

    fs::remove_file(root.join("loose.wav")).unwrap();
    let scope = Scope::folder(&root, &root, Depth::Directory);
    let scan = scan_once(&store, &lib, Trigger::Watch, vec![scope]);
    assert_eq!(scan.progress.missing, 1);
    let live: Vec<PathBuf> = live(&store, &lib).into_iter().map(|t| t.path).collect();
    assert_eq!(live, [root.join("A/X/01 - Track 1.wav")]);
}

/// A scan interrupted mid-way resumes from its cursor when the scanner starts. Work that a
/// queued scan already covers joins it instead of queuing again, while work the running scan
/// covers queues behind it, since the walk may already have passed it.
pub fn queue_runs_resumes_and_coalesces<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    make_album(&root, "B", "Y", 2);
    make_album(&root, "C", "Z", 2);
    let store: Arc<S> = Arc::new(make(&root));
    let lib = library(&root);

    // A scan interrupted after finishing directory "A/X" is left Running
    // with a cursor. Starting the scanner resumes it.
    let interrupted = Scan {
        id: 0,
        library: lib.id,
        trigger: Trigger::Initial,
        scopes: vec![Scope::root(&root)],
        state: ScanState::Running,
        started_at: Some(std::time::SystemTime::now()),
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: Some(Cursor { scope_index: 0, after_directory: Some(root.join("A/X")) }),
    };
    let mut interrupted_row = interrupted.clone();
    interrupted_row.id = store.create_scan(&interrupted).unwrap();
    store.update_scan(&interrupted_row).unwrap();

    let governor = Arc::new(Governor::new());
    let scanner =
        Scanner::start(store.clone(), lib.clone(), governor.clone(), ScanOptions::default());
    scanner.wait_idle();
    let scans = store.inspect_scans(lib.id);
    let resumed = scans.iter().find(|s| s.id == interrupted_row.id).unwrap();
    assert_eq!(resumed.state, ScanState::Completed);
    assert_eq!(resumed.progress.files_seen, 4, "{:?}", resumed.progress);

    // A scan of "C", parked by the paused governor, holds what is requested after it in the queue.
    governor.pause();
    let held = scanner
        .request(Trigger::Manual, vec![Scope::folder(&root, root.join("C"), Depth::Subtree)])
        .unwrap();
    wait_until_running(&scanner, held);
    let z = || vec![Scope::folder(&root, root.join("C/Z"), Depth::Directory)];
    let behind = scanner.request(Trigger::Watch, z()).unwrap();
    assert_ne!(behind, held, "the running scan takes no new work, even work it covers");
    assert_eq!(
        scanner.request(Trigger::Watch, z()).unwrap(),
        behind,
        "it joins the one queued behind"
    );
    let dropped = scanner
        .request(Trigger::Manual, vec![Scope::folder(&root, root.join("B"), Depth::Subtree)])
        .unwrap();
    assert!(scanner.cancel(dropped));
    assert!(scanner.queued().iter().all(|s| s.id != dropped), "{:?}", scanner.queued());
    assert!(!scanner.cancel(dropped), "a cancelled scan is gone");
    let stored = store.inspect_scans(lib.id).into_iter().find(|s| s.id == dropped).unwrap();
    assert_eq!(stored.state, ScanState::Cancelled);
    assert!(stored.finished_at.is_some());
    let a = scanner
        .request(Trigger::Manual, vec![Scope::folder(&root, root.join("A"), Depth::Subtree)])
        .unwrap();
    let b = scanner
        .request(Trigger::Manual, vec![Scope::folder(&root, root.join("A/X"), Depth::Directory)])
        .unwrap();
    let all = scanner.scan_library(Trigger::Scheduled).unwrap();
    let again = scanner.scan_library(Trigger::Manual).unwrap();
    assert_eq!(a, b, "a covered scope joins the queued scan");
    assert_eq!(all, again);
    governor.resume();
    scanner.wait_idle();
    let final_scans = store.inspect_scans(lib.id);
    let state = |id| final_scans.iter().find(|s| s.id == id).unwrap().state;
    assert_eq!(state(held), ScanState::Completed);
    assert_eq!(state(behind), ScanState::Cancelled, "the library scan replaced it");
    assert_eq!(state(a), ScanState::Cancelled, "the library scan replaced it");
    assert_eq!(state(all), ScanState::Completed);
    assert_eq!(live(store.as_ref(), &lib).len(), 6);
    scanner.shutdown();
}

/// Blocks until `condition` holds for `scanner`, failing the test with `what` after 30 seconds.
pub fn wait_until(scanner: &Scanner, what: &str, condition: impl Fn(&Scanner) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition(scanner) {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Blocks until `scanner` is running the scan `id`, failing the test after 30 seconds.
fn wait_until_running(scanner: &Scanner, id: ScanId) {
    let started = |scanner: &Scanner| scanner.running().is_some_and(|scan| scan.id == id);
    wait_until(scanner, &format!("scan {id} never started"), started);
}

/// A resumed scan waiting in the queue takes no new work, since its walk has passed what lies
/// before its cursor: the startup scan queues anew, so a file added there while the server was
/// down is indexed. Two library scans interrupted after "B/Y" are resumed; whichever runs first
/// is parked by the paused governor, and the other waits in the queue.
pub fn a_resumed_scan_takes_no_new_work<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "B", "Y", 1);
    make_album(&root, "C", "Z", 1);
    let store: Arc<S> = Arc::new(make(&root));
    let lib = library(&root);
    for _ in 0..2 {
        let mut interrupted = Scan {
            id: 0,
            library: lib.id,
            trigger: Trigger::Initial,
            scopes: vec![Scope::root(&root)],
            state: ScanState::Running,
            started_at: Some(std::time::SystemTime::now()),
            finished_at: None,
            progress: ScanProgress::default(),
            cursor: Some(Cursor { scope_index: 0, after_directory: Some(root.join("B/Y")) }),
        };
        interrupted.id = store.create_scan(&interrupted).unwrap();
        store.update_scan(&interrupted).unwrap();
    }

    let governor = Arc::new(Governor::new());
    governor.pause();
    let scanner =
        Scanner::start(store.clone(), lib.clone(), governor.clone(), ScanOptions::default());
    wait_until(&scanner, "no resumed scan started", |scanner| scanner.running().is_some());
    let [waiting] = scanner.queued().try_into().unwrap();
    let added = root.join("A/X/02 - Added.wav");
    write_small_wav(&added);
    let startup = scanner.scan_library(Trigger::Initial).unwrap();
    assert_ne!(startup, waiting.id, "the startup scan does not join a resumed scan");
    governor.resume();
    scanner.wait_idle();
    scanner.shutdown();

    assert!(live(store.as_ref(), &lib).iter().any(|t| t.path == added));
    let scans = store.inspect_scans(lib.id);
    let state = |id| scans.iter().find(|s| s.id == id).unwrap().state;
    assert_eq!(state(waiting.id), ScanState::Cancelled, "the startup scan replaced it");
    assert_eq!(state(startup), ScanState::Completed);
}

/// A reconfiguration while a library scan runs queues a scan of its own, which applies the
/// new excludes: tracks under an added exclude end up missing. The library is indexed first,
/// so the outcome holds whichever configuration the parked scan read when it started.
pub fn a_reconfigure_during_a_scan_applies_its_excludes<S: InspectStore + 'static>(
    make: Factory<S>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "backup", "X", 1);
    let store: Arc<S> = Arc::new(make(&root));
    let lib = library(&root);
    let governor = Arc::new(Governor::new());
    let scanner =
        Scanner::start(store.clone(), lib.clone(), governor.clone(), ScanOptions::default());
    scanner.scan_library(Trigger::Initial).unwrap();
    scanner.wait_idle();
    governor.pause();
    let running = scanner.scan_library(Trigger::Scheduled).unwrap();
    wait_until_running(&scanner, running);

    let excluding = LibraryConfig { excludes: vec!["backup".into(), "backup/**".into()], ..lib };
    let reconfigured = scanner.reconfigure(excluding.clone()).unwrap();
    assert_ne!(reconfigured, running, "a scan that has started takes no new work");
    governor.resume();
    scanner.wait_idle();
    scanner.shutdown();

    let tracks = store.inspect_tracks(excluding.id);
    let missing = |path: &str| tracks.iter().find(|t| t.path == root.join(path)).unwrap().missing;
    assert!(!missing("A/X/01 - Track 1.wav"));
    assert!(missing("backup/X/01 - Track 1.wav"));
}

/// Generate one `#[test]` per scenario against a store factory.
///
pub fn placeholders_fill_once_per_image<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    // Two tracks share a red 300×200 cover; another album's cover has a valid header and
    // nothing after it, so the scanner indexes it and ffmpeg cannot decode it.
    make_album(&root, "A", "X", 2);
    make_album(&root, "B", "Y", 1);
    let cover = root.join("A/X/cover.png");
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=red:s=300x200"])
        .args(["-frames:v", "1"])
        .arg(&cover)
        .status()
        .unwrap();
    assert!(status.success());
    let png = fs::read(&cover).unwrap();
    fs::write(root.join("B/Y/cover.png"), &png[..33]).unwrap();
    let store = Arc::new(make(&root));
    let lib = library(&root);
    scan_library(&*store, &lib);

    let pending = store.next_without_placeholder(lib.id, 10).unwrap();
    assert_eq!(pending.len(), 2, "one per distinct image: {pending:?}");
    let red = pending.iter().find(|(_, file)| file.path == cover).unwrap().0.clone();
    let broken = pending.iter().find(|(hash, _)| *hash != red).unwrap().0.clone();

    let ffmpeg = jewelcase_ffmpeg::Ffmpeg::new(jewelcase_ffmpeg::Config::default());
    let placeholders = crate::placeholders::Placeholders::new(ffmpeg, store.clone());
    let report = placeholders.run_once(lib.id, 10);
    assert_eq!((report.done, report.failed), (1, 1));
    assert!(store.next_without_placeholder(lib.id, 10).unwrap().is_empty());
    assert!(placeholders.run_once(lib.id, 10).idle(), "a broken image is not retried");

    let hash = store.inspect_placeholder(lib.id, &red).unwrap();
    assert!(hash.len() < 40, "a few bytes: {}", hash.len());
    let (r, g, b, a) = thumbhash::thumb_hash_to_average_rgba(&hash).unwrap();
    assert!(r > 0.9 && g < 0.1 && b < 0.1 && a > 0.99, "{r} {g} {b} {a}");
    let ratio = thumbhash::thumb_hash_to_approximate_aspect_ratio(&hash).unwrap();
    // ThumbHash keeps the aspect ratio only coarsely.
    assert!((ratio - 1.5).abs() < 0.2, "{ratio}");
    assert_eq!(store.inspect_placeholder(lib.id, &broken), Some(Vec::new()));
}

/// The one track [`rescan_after`] indexes, relative to the library root.
const TRACK: &str = "A/X/01 - Track 1.wav";
/// Where the move, copy and replace scenarios put a file.
const ELSEWHERE: &str = "B/Y/01 - Track 1.wav";

/// Index one track at [`TRACK`], apply `change` to the library root, and scan again.
/// Returns the track as first indexed, the second scan, every track after it, missing
/// ones included, and the tracks the second scan announced in the feed.
fn rescan_after<S: InspectStore>(
    make: Factory<S>,
    change: impl FnOnce(&Path),
) -> (InspectTrack, Scan, Vec<InspectTrack>, Vec<TrackId>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let store = make(&root);
    let lib = library(&root);
    scan_library(&store, &lib);
    let before = live(&store, &lib)[0].clone();
    let head = store.feed_head(lib.id);
    change(&root);
    let scan = scan_library(&store, &lib);
    (before, scan, store.inspect_tracks(lib.id), store.feed_tracks_after(lib.id, head))
}

/// Move [`TRACK`] under `root` to [`ELSEWHERE`], returning its new path.
fn move_track(root: &Path) -> PathBuf {
    let moved = root.join(ELSEWHERE);
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(root.join(TRACK), &moved).unwrap();
    moved
}

/// Assert that `scan` found `before` again at [`ELSEWHERE`], adding and losing nothing,
/// and return it as it is now.
fn assert_found_moved<'a>(
    before: &InspectTrack,
    scan: &Scan,
    tracks: &'a [InspectTrack],
) -> &'a InspectTrack {
    assert_eq!(scan.progress.added, 0);
    assert_eq!(scan.progress.missing, 0);
    let [after] = tracks else { panic!("one track, not {tracks:?}") };
    assert!(!after.missing);
    assert!(after.path.ends_with(ELSEWHERE), "{:?}", after.path);
    assert_eq!(after.id, before.id);
    after
}

/// A file moved to another folder is the same track in its new place
/// (`requirements/scanning.md` §6).
pub fn move_keeps_identity<S: InspectStore>(make: Factory<S>) {
    let (before, scan, tracks, announced) = rescan_after(make, |root| {
        move_track(root);
    });
    assert_found_moved(&before, &scan, &tracks);
    assert!(announced.iter().all(|id| *id == before.id), "{announced:?}");
}

/// A file both moved and retagged before the next scan is still the same track, with
/// its new tags (`requirements/scanning.md` §6). The new title changes the file's size, so
/// matching on size alone does not pass.
pub fn move_and_retag_keeps_identity<S: InspectStore>(make: Factory<S>) {
    const TITLE: &str = "A title much longer than the one it replaces";
    let (before, scan, tracks, announced) = rescan_after(make, |root| {
        let moved = move_track(root);
        let size = fs::metadata(&moved).unwrap().len();
        tag_wav(&moved, TITLE, "B", "Y");
        assert_ne!(fs::metadata(&moved).unwrap().len(), size, "the retag changes the size");
        fs::File::open(&moved)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
            .unwrap();
    });
    let after = assert_found_moved(&before, &scan, &tracks);
    assert_eq!(after.title, TITLE);
    assert_eq!(announced, vec![before.id]);
}

/// A copy of a file is a second track, and the original keeps its identity
/// (`requirements/scanning.md` §9).
pub fn copy_is_a_second_track<S: InspectStore>(make: Factory<S>) {
    let (before, scan, tracks, _) = rescan_after(make, |root| {
        let copy = root.join(ELSEWHERE);
        fs::create_dir_all(copy.parent().unwrap()).unwrap();
        fs::copy(root.join(TRACK), copy).unwrap();
    });
    assert_eq!(scan.progress.added, 1);
    assert_eq!(scan.progress.missing, 0);
    assert!(tracks.iter().all(|t| !t.missing), "{tracks:?}");
    assert_eq!(tracks.len(), 2, "{tracks:?}");
    let at = |place: &str| tracks.iter().find(|t| t.path.ends_with(place)).unwrap();
    let (original, copy) = (at(TRACK), at(ELSEWHERE));
    assert_eq!(original.id, before.id);
    assert_ne!(copy.id, before.id);
}

/// A file deleted while different audio appears elsewhere is a loss and a new track, not a
/// move: the vanished track's identity does not pass to a file with other content
/// (`requirements/scanning.md` §6).
pub fn replaced_is_not_a_move<S: InspectStore>(make: Factory<S>) {
    let (before, scan, tracks, _) = rescan_after(make, |root| {
        fs::remove_file(root.join(TRACK)).unwrap();
        write_sine_wav(&root.join(ELSEWHERE), 880.0, 0.3, 0.5);
    });
    assert_eq!(scan.progress.added, 1);
    assert_eq!(scan.progress.missing, 1);
    let missing: Vec<&InspectTrack> = tracks.iter().filter(|t| t.missing).collect();
    assert_eq!(missing.len(), 1, "{tracks:?}");
    assert_eq!(missing[0].id, before.id);
}

/// ```ignore
/// jewelcase_scanner::store_suite!(|_root| MemoryStore::new());
/// ```
#[macro_export]
macro_rules! store_suite {
    ($make:expr) => {
        #[test]
        fn cold_scan_then_incremental_is_all_unchanged() {
            $crate::testing::cold_scan_then_incremental_is_all_unchanged($make);
        }
        #[test]
        fn retag_updates_in_place_and_keeps_identity() {
            $crate::testing::retag_updates_in_place_and_keeps_identity($make);
        }
        #[test]
        fn lyrics_file_beside_an_unchanged_track() {
            $crate::testing::lyrics_file_beside_an_unchanged_track($make);
        }
        #[test]
        fn a_changed_file_is_analyzed_again() {
            $crate::testing::a_changed_file_is_analyzed_again($make);
        }
        #[test]
        fn a_lyrics_file_alone_keeps_the_analysis() {
            $crate::testing::a_lyrics_file_alone_keeps_the_analysis($make);
        }
        #[test]
        fn a_result_for_replaced_audio_is_not_stored() {
            $crate::testing::a_result_for_replaced_audio_is_not_stored($make);
        }
        #[test]
        fn a_known_file_that_cannot_be_read_keeps_its_track() {
            $crate::testing::a_known_file_that_cannot_be_read_keeps_its_track($make);
        }
        #[test]
        fn an_unreadable_file_is_read_again_until_it_reads() {
            $crate::testing::an_unreadable_file_is_read_again_until_it_reads($make);
        }
        #[test]
        fn a_broken_file_moved_away_and_back_keeps_its_problem() {
            $crate::testing::a_broken_file_moved_away_and_back_keeps_its_problem($make);
        }
        #[test]
        fn a_file_restored_with_its_old_time_is_read_again() {
            $crate::testing::a_file_restored_with_its_old_time_is_read_again($make);
        }
        #[test]
        fn a_result_for_a_file_broken_during_its_decode_is_not_stored() {
            $crate::testing::a_result_for_a_file_broken_during_its_decode_is_not_stored($make);
        }
        #[test]
        fn an_unreadable_replacement_loses_its_analysis() {
            $crate::testing::an_unreadable_replacement_loses_its_analysis($make);
        }
        #[test]
        fn a_missing_file_back_unreadable_is_back() {
            $crate::testing::a_missing_file_back_unreadable_is_back($make);
        }
        #[test]
        fn missing_then_returned() {
            $crate::testing::missing_then_returned($make);
        }
        #[test]
        fn unavailable_root_suspends_and_touches_nothing() {
            $crate::testing::unavailable_root_suspends_and_touches_nothing($make);
        }
        #[test]
        fn bad_file_is_a_problem_that_clears() {
            $crate::testing::bad_file_is_a_problem_that_clears($make);
        }
        #[test]
        fn a_symlink_is_a_problem() {
            $crate::testing::a_symlink_is_a_problem($make);
        }
        #[test]
        fn a_scope_through_a_folder_link_walks_nothing() {
            $crate::testing::a_scope_through_a_folder_link_walks_nothing($make);
        }
        #[test]
        fn a_scope_naming_a_deleted_folder_reconciles() {
            $crate::testing::a_scope_naming_a_deleted_folder_reconciles($make);
        }
        #[test]
        fn a_deleted_file_takes_its_problem() {
            $crate::testing::a_deleted_file_takes_its_problem($make);
        }
        #[test]
        fn a_scope_at_a_file_reconciles_nothing() {
            $crate::testing::a_scope_at_a_file_reconciles_nothing($make);
        }
        #[test]
        fn indexes_under_sees_live_tracks_and_problems() {
            $crate::testing::indexes_under_sees_live_tracks_and_problems($make);
        }
        #[test]
        fn a_dangling_link_keeps_its_problem() {
            $crate::testing::a_dangling_link_keeps_its_problem($make);
        }
        #[test]
        fn a_problem_on_a_present_file_stays() {
            $crate::testing::a_problem_on_a_present_file_stays($make);
        }
        #[test]
        fn a_folder_problem_clears_once_it_lists() {
            $crate::testing::a_folder_problem_clears_once_it_lists($make);
        }
        #[test]
        fn an_unreadable_folder_keeps_its_tracks() {
            $crate::testing::an_unreadable_folder_keeps_its_tracks($make);
        }
        #[test]
        fn untagged_file_is_named_after_itself() {
            $crate::testing::untagged_file_is_named_after_itself($make);
        }
        #[test]
        fn excludes_are_never_indexed() {
            $crate::testing::excludes_are_never_indexed($make);
        }
        #[test]
        fn folder_scope_reconciles_only_inside_itself() {
            $crate::testing::folder_scope_reconciles_only_inside_itself($make);
        }
        #[test]
        fn non_ascii_subtree_reconciles_only_inside_itself() {
            $crate::testing::non_ascii_subtree_reconciles_only_inside_itself($make);
        }
        #[test]
        fn non_ascii_directory_reconciles_only_itself() {
            $crate::testing::non_ascii_directory_reconciles_only_itself($make);
        }
        #[test]
        fn root_directory_reconciles_only_its_own_files() {
            $crate::testing::root_directory_reconciles_only_its_own_files($make);
        }
        #[test]
        fn queue_runs_resumes_and_coalesces() {
            $crate::testing::queue_runs_resumes_and_coalesces($make);
        }
        #[test]
        fn a_resumed_scan_takes_no_new_work() {
            $crate::testing::a_resumed_scan_takes_no_new_work($make);
        }
        #[test]
        fn a_reconfigure_during_a_scan_applies_its_excludes() {
            $crate::testing::a_reconfigure_during_a_scan_applies_its_excludes($make);
        }
        #[test]
        fn a_shutdown_leaves_its_scans_to_resume() {
            $crate::testing::a_shutdown_leaves_its_scans_to_resume($make);
        }
        #[test]
        fn a_cancelled_scan_stays_cancelled() {
            $crate::testing::a_cancelled_scan_stays_cancelled($make);
        }
        #[test]
        fn placeholders_fill_once_per_image() {
            $crate::testing::placeholders_fill_once_per_image($make);
        }
        #[test]
        fn a_missing_file_waits_until_it_returns() {
            $crate::testing::a_missing_file_waits_until_it_returns($make);
        }
        #[test]
        fn a_file_not_permitted_waits_until_it_is() {
            $crate::testing::a_file_not_permitted_waits_until_it_is($make);
        }
        #[test]
        fn an_older_analyzers_results_are_redone() {
            $crate::testing::an_older_analyzers_results_are_redone($make);
        }
        #[test]
        fn an_unopenable_file_does_not_starve_the_next() {
            $crate::testing::an_unopenable_file_does_not_starve_the_next($make);
        }
        #[test]
        fn opening_a_file_clears_only_unopened_problems() {
            $crate::testing::opening_a_file_clears_only_unopened_problems($make);
        }
        #[test]
        fn a_cover_moved_away_waits_until_it_returns() {
            $crate::testing::a_cover_moved_away_waits_until_it_returns($make);
        }
        #[test]
        fn a_pipe_is_judged_without_blocking() {
            $crate::testing::a_pipe_is_judged_without_blocking($make);
        }
        #[test]
        fn a_decoder_that_cannot_start_backs_off() {
            $crate::testing::a_decoder_that_cannot_start_backs_off($make);
        }
        #[test]
        fn a_root_going_away_pauses_without_writing() {
            $crate::testing::a_root_going_away_pauses_without_writing($make);
        }
        #[test]
        fn a_file_that_always_fails_does_not_block_the_next() {
            $crate::testing::a_file_that_always_fails_does_not_block_the_next($make);
        }
        #[test]
        fn a_link_is_judged_once() {
            $crate::testing::a_link_is_judged_once($make);
        }
        #[test]
        fn copy_is_a_second_track() {
            $crate::testing::copy_is_a_second_track($make);
        }
        #[test]
        fn replaced_is_not_a_move() {
            $crate::testing::replaced_is_not_a_move($make);
        }
        #[test]
        #[ignore = "track identity across moves is open decision 3 (design/general.md §11)"]
        fn move_keeps_identity() {
            $crate::testing::move_keeps_identity($make);
        }
        #[test]
        #[ignore = "track identity across moves is open decision 3 (design/general.md §11)"]
        fn move_and_retag_keeps_identity() {
            $crate::testing::move_and_retag_keeps_identity($make);
        }
    };
}
