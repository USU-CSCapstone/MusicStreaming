//! Fixtures and the store conformance suite (`design/scanning.md` §15).
//!
//! Every [`Store`] implementation runs the same scenarios through the real
//! pipeline. The in-memory store passes them here; the server's SQLite store
//! runs them through [`store_suite!`] and must pass unchanged.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use jewelcase_core::Format;
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag, TagType};

use crate::governor::Governor;
use crate::queue::Scanner;
use crate::scan::{ScanContext, ScanOptions, run_scan};
use crate::store::{MemoryStore, Store};
use crate::types::*;

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
}

/// What a store must expose for the suite to check it.
pub trait InspectStore: Store {
    fn inspect_tracks(&self, library: &LibraryId) -> Vec<InspectTrack>;
    fn inspect_problems(&self, library: &LibraryId) -> Vec<Problem>;
    fn inspect_scans(&self, library: &LibraryId) -> Vec<Scan>;
    /// Number of live change-feed rows. Only compared before and after.
    fn feed_len(&self, library: &LibraryId) -> usize;
}

impl InspectStore for MemoryStore {
    fn inspect_tracks(&self, library: &LibraryId) -> Vec<InspectTrack> {
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
            })
            .collect()
    }

    fn inspect_problems(&self, library: &LibraryId) -> Vec<Problem> {
        self.problems(library)
    }

    fn inspect_scans(&self, library: &LibraryId) -> Vec<Scan> {
        self.scans(library)
    }

    fn feed_len(&self, library: &LibraryId) -> usize {
        self.feed(library).len()
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
        tag.push(lofty::tag::TagItem::new(
            *key,
            lofty::tag::ItemValue::Text((*value).to_owned()),
        ));
    }
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

pub fn make_album(root: &Path, artist: &str, album: &str, n: usize) {
    for i in 1..=n {
        let p = root
            .join(artist)
            .join(album)
            .join(format!("{i:02} - Track {i}.wav"));
        write_small_wav(&p);
        tag_wav(&p, &format!("Track {i}"), artist, album);
    }
}

pub fn library(root: &Path) -> LibraryConfig {
    LibraryConfig {
        id: "1".into(),
        roots: vec![root.to_path_buf()],
        excludes: Vec::new(),
    }
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
        library: lib.id.clone(),
        trigger,
        scopes,
        state: ScanState::Queued,
        started_at: None,
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: None,
    };
    scan.id = store.create_scan(&scan);
    let ctx = ScanContext {
        store,
        library: lib,
        governor: &governor,
        cancel: &cancel,
        options: &options,
    };
    run_scan(&ctx, &mut scan);
    scan
}

pub fn scan_library(store: &dyn Store, lib: &LibraryConfig) -> Scan {
    scan_once(
        store,
        lib,
        Trigger::Manual,
        lib.roots.iter().map(Scope::root).collect(),
    )
}

fn live(store: &dyn InspectStore, lib: &LibraryConfig) -> Vec<InspectTrack> {
    let mut v: Vec<InspectTrack> = store
        .inspect_tracks(&lib.id)
        .into_iter()
        .filter(|t| !t.missing)
        .collect();
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
    fs::write(
        root.join("Nirvana/Nevermind/01 - Track 1.lrc"),
        "[00:01.00]hi",
    )
    .unwrap();
    fs::write(root.join("readme.txt"), "ignored").unwrap();
    fs::write(root.join("Nirvana/old.wma"), "ignored too").unwrap();

    let store = make(&root);
    let lib = library(&root);

    let first = scan_library(&store, &lib);
    assert_eq!(first.state, ScanState::Completed);
    assert_eq!(first.progress.files_seen, 5);
    assert_eq!(first.progress.added, 5);
    assert_eq!(
        first.progress.problems,
        0,
        "{:?}",
        store.inspect_problems(&lib.id)
    );
    let tracks = live(&store, &lib);
    assert_eq!(tracks.len(), 5);

    let t1 = tracks
        .iter()
        .find(|t| t.path.ends_with("Nevermind/01 - Track 1.wav"))
        .unwrap();
    assert!(t1.has_artwork, "cover.png in the album folder");
    assert!(t1.has_biography, "artist.txt one level up");
    assert!(t1.has_sidecar_lyrics, "lyrics by stem");
    assert_eq!(t1.title, "Track 1");
    assert_eq!(t1.artists, vec!["Nirvana"]);
    assert_eq!(t1.sort_artist, "nirvana");
    let utero = tracks
        .iter()
        .find(|t| t.path.ends_with("In Utero/01 - Track 1.wav"))
        .unwrap();
    assert!(!utero.has_artwork, "no cover anywhere above In Utero");
    assert!(
        utero.has_biography,
        "biography inherited from the artist folder"
    );

    let feed_after_first = store.feed_len(&lib.id);
    assert!(feed_after_first >= 5, "at least one feed row per track");

    // Second scan: nothing opened, nothing changed.
    let second = scan_library(&store, &lib);
    assert_eq!(second.state, ScanState::Completed);
    assert_eq!(second.progress.files_seen, 5);
    assert_eq!(second.progress.files_processed, 0);
    assert_eq!(second.progress.added + second.progress.updated, 0);
    assert_eq!(second.progress.missing, 0);
    assert_eq!(
        store.feed_len(&lib.id),
        feed_after_first,
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
    fs::remove_file(&p).unwrap();

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.missing, 1);
    let missing: Vec<InspectTrack> = store
        .inspect_tracks(&lib.id)
        .into_iter()
        .filter(|t| t.missing)
        .collect();
    assert_eq!(missing.len(), 1);
    assert!(missing[0].path.ends_with("02 - Track 2.wav"));
    let id = missing[0].id;
    let feed_before_return = store.feed_len(&lib.id);

    // The file comes back byte-identical with its old mtime: reconnects.
    fs::write(&p, &bytes).unwrap();
    fs::File::open(&p)
        .unwrap()
        .set_modified(meta.modified().unwrap())
        .unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.missing, 0);
    assert_eq!(scan.progress.added, 0);
    let t = store
        .inspect_tracks(&lib.id)
        .into_iter()
        .find(|t| t.path == p)
        .unwrap();
    assert!(!t.missing);
    assert_eq!(t.id, id);
    assert!(
        store.feed_len(&lib.id) >= feed_before_return,
        "the return is announced"
    );
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
    assert!(store.inspect_tracks(&lib.id).iter().all(|t| !t.missing));

    // An empty mount point is unavailable too.
    fs::create_dir(&root).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Suspended);
    assert!(store.inspect_tracks(&lib.id).iter().all(|t| !t.missing));

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
    assert_eq!(
        scan.state,
        ScanState::Completed,
        "one bad file never aborts a scan"
    );
    assert_eq!(scan.progress.added, 1);
    assert_eq!(scan.progress.problems, 1);
    let problems = store.inspect_problems(&lib.id);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path, bad);
    assert_eq!(problems[0].kind, ProblemKind::CorruptAudio);

    // Replace with a real file: the problem clears.
    write_small_wav(&bad);
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.added, 1);
    assert!(store.inspect_problems(&lib.id).is_empty());
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
    assert!(store.inspect_tracks(&lib.id).iter().all(|t| !t.missing));
    let scan = scan_once(
        &store,
        &lib,
        Trigger::Manual,
        vec![Scope::folder(&root, root.join("B"), Depth::Subtree)],
    );
    assert_eq!(scan.progress.missing, 1);
}

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
        library: lib.id.clone(),
        trigger: Trigger::Initial,
        scopes: vec![Scope::root(&root)],
        state: ScanState::Running,
        started_at: Some(std::time::SystemTime::now()),
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: Some(Cursor {
            scope_index: 0,
            after_directory: Some(root.join("A/X")),
        }),
    };
    let mut interrupted_row = interrupted.clone();
    interrupted_row.id = store.create_scan(&interrupted);
    store.update_scan(&interrupted_row);

    let governor = Arc::new(Governor::new());
    let scanner = Scanner::start(store.clone(), lib.clone(), governor, ScanOptions::default());
    scanner.wait_idle();
    let scans = store.inspect_scans(&lib.id);
    let resumed = scans.iter().find(|s| s.id == interrupted_row.id).unwrap();
    assert_eq!(resumed.state, ScanState::Completed);
    assert_eq!(resumed.progress.files_seen, 4, "{:?}", resumed.progress);

    let a = scanner.request(
        Trigger::Manual,
        vec![Scope::folder(&root, root.join("A"), Depth::Subtree)],
    );
    let b = scanner.request(
        Trigger::Manual,
        vec![Scope::folder(&root, root.join("A/X"), Depth::Directory)],
    );
    let all = scanner.scan_library(Trigger::Scheduled);
    let again = scanner.scan_library(Trigger::Manual);
    assert_eq!(a, b, "a covered scope joins the existing scan");
    assert_eq!(all, again);
    scanner.wait_idle();
    let final_scans = store.inspect_scans(&lib.id);
    let a_scan = final_scans.iter().find(|s| s.id == a).unwrap();
    assert!(matches!(
        a_scan.state,
        ScanState::Completed | ScanState::Cancelled
    ));
    assert_eq!(
        final_scans.iter().find(|s| s.id == all).unwrap().state,
        ScanState::Completed
    );
    assert_eq!(live(store.as_ref(), &lib).len(), 6);
    scanner.shutdown();
}

/// Generate one `#[test]` per scenario against a store factory.
///
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
        fn queue_runs_resumes_and_coalesces() {
            $crate::testing::queue_runs_resumes_and_coalesces($make);
        }
    };
}
