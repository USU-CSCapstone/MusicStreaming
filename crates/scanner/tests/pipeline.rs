//! The pipeline end to end on a temporary library with the in-memory store.

mod common;

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use jewelcase_scanner::scan::ScanOptions;
use jewelcase_scanner::store::FeedEntry;
use jewelcase_scanner::*;
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag, TagType};

fn tag_wav(path: &std::path::Path, title: &str, artist: &str, album: &str) {
    let mut tag = Tag::new(TagType::Id3v2);
    tag.insert_text(ItemKey::TrackTitle, title.into());
    tag.insert_text(ItemKey::TrackArtist, artist.into());
    tag.insert_text(ItemKey::AlbumTitle, album.into());
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

fn make_album(root: &std::path::Path, artist: &str, album: &str, n: usize) {
    for i in 1..=n {
        let p = root
            .join(artist)
            .join(album)
            .join(format!("{i:02} - Track {i}.wav"));
        write_small_wav(&p);
        tag_wav(&p, &format!("Track {i}"), artist, album);
    }
}

#[test]
fn cold_scan_then_incremental_is_all_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    make_album(root, "Nirvana", "Nevermind", 3);
    make_album(root, "Nirvana", "In Utero", 2);
    fs::write(root.join("Nirvana/artist.txt"), "bio").unwrap();
    fs::write(
        root.join("Nirvana/Nevermind/cover.jpg"),
        b"not really a jpeg",
    )
    .unwrap();
    fs::write(
        root.join("Nirvana/Nevermind/01 - Track 1.lrc"),
        "[00:01.00]hi",
    )
    .unwrap();
    fs::write(root.join("readme.txt"), "ignored").unwrap();
    fs::write(root.join("Nirvana/old.wma"), "ignored too").unwrap();

    let store = MemoryStore::new();
    let lib = library(root);

    let first = scan_library(&store, &lib);
    assert_eq!(first.state, ScanState::Completed);
    assert_eq!(first.progress.files_seen, 5);
    assert_eq!(first.progress.added, 5);
    assert_eq!(first.progress.problems, 0, "{:?}", store.problems(&lib.id));
    assert_eq!(paths_of(&store, &lib).len(), 5);

    // Sidecars attached, nearest wins, lyrics by stem.
    let t1 = store
        .tracks(&lib.id)
        .into_iter()
        .find(|t| t.record.path.ends_with("Nevermind/01 - Track 1.wav"))
        .unwrap()
        .record;
    assert_eq!(
        t1.artwork,
        Some(ArtworkSource::Sidecar(
            root.join("Nirvana/Nevermind/cover.jpg")
        ))
    );
    assert_eq!(t1.artist_biography, Some(root.join("Nirvana/artist.txt")));
    assert_eq!(
        t1.lyrics_sidecar,
        Some(root.join("Nirvana/Nevermind/01 - Track 1.lrc"))
    );
    assert_eq!(t1.tags.title.as_deref(), Some("Track 1"));
    assert_eq!(t1.tags.artists, vec!["Nirvana"]);
    assert_eq!(t1.sort.artist, "nirvana");
    let utero = store
        .tracks(&lib.id)
        .into_iter()
        .find(|t| t.record.path.ends_with("In Utero/01 - Track 1.wav"))
        .unwrap();
    assert_eq!(utero.record.artwork, None);
    assert_eq!(
        utero.record.artist_biography,
        Some(root.join("Nirvana/artist.txt"))
    );

    // Feed carried one add per track.
    assert_eq!(
        store
            .feed(&lib.id)
            .iter()
            .filter(|e| matches!(e, FeedEntry::Added(_)))
            .count(),
        5
    );

    // Second scan: nothing opened, nothing changed.
    let second = scan_library(&store, &lib);
    assert_eq!(second.state, ScanState::Completed);
    assert_eq!(second.progress.files_seen, 5);
    assert_eq!(second.progress.files_processed, 0);
    assert_eq!(second.progress.added + second.progress.updated, 0);
    assert_eq!(second.progress.missing, 0);
    assert_eq!(
        store.feed(&lib.id).len(),
        5,
        "an unchanged scan must not touch the feed"
    );
}

#[test]
fn retag_updates_in_place_and_keeps_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    make_album(root, "A", "X", 1);
    let store = MemoryStore::new();
    let lib = library(root);
    scan_library(&store, &lib);
    let before = store.tracks(&lib.id)[0].clone();

    // Ensure the mtime moves even on coarse filesystems.
    std::thread::sleep(Duration::from_millis(20));
    let p = root.join("A/X/01 - Track 1.wav");
    tag_wav(&p, "Renamed", "A", "X");
    let ft = fs::File::open(&p).unwrap();
    ft.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.updated, 1);
    let after = store.tracks(&lib.id)[0].clone();
    assert_eq!(after.record.id, before.record.id);
    assert_eq!(after.record.tags.title.as_deref(), Some("Renamed"));
}

#[test]
fn missing_then_returned() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    make_album(root, "A", "X", 2);
    let store = MemoryStore::new();
    let lib = library(root);
    scan_library(&store, &lib);

    let p = root.join("A/X/02 - Track 2.wav");
    let bytes = fs::read(&p).unwrap();
    let meta = fs::metadata(&p).unwrap();
    fs::remove_file(&p).unwrap();

    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.missing, 1);
    let missing: Vec<_> = store
        .tracks(&lib.id)
        .into_iter()
        .filter(|t| t.missing)
        .collect();
    assert_eq!(missing.len(), 1);
    assert!(missing[0].record.path.ends_with("02 - Track 2.wav"));
    let id = missing[0].record.id;

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
        .tracks(&lib.id)
        .into_iter()
        .find(|t| t.record.path == p)
        .unwrap();
    assert!(!t.missing);
    assert_eq!(t.record.id, id);
    assert!(
        store
            .feed(&lib.id)
            .contains(&FeedEntry::Returned(id.unwrap()))
    );
}

#[test]
fn unavailable_root_suspends_and_touches_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("nas");
    make_album(&root, "A", "X", 2);
    let store = MemoryStore::new();
    let lib = library(&root);
    scan_library(&store, &lib);
    assert_eq!(paths_of(&store, &lib).len(), 2);

    // "Unmount": the root vanishes.
    let stash = tmp.path().join("stash");
    fs::rename(&root, &stash).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Suspended);
    assert_eq!(scan.progress.missing, 0);
    assert!(store.tracks(&lib.id).iter().all(|t| !t.missing));

    // An empty mount point is unavailable too.
    fs::create_dir(&root).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Suspended);
    assert!(store.tracks(&lib.id).iter().all(|t| !t.missing));

    // Remount: everything unchanged, nothing re-added.
    fs::remove_dir(&root).unwrap();
    fs::rename(&stash, &root).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.state, ScanState::Completed);
    assert_eq!(scan.progress.added, 0);
}

#[test]
fn bad_file_is_a_problem_that_clears() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    make_album(root, "A", "X", 1);
    let bad = root.join("A/X/broken.flac");
    fs::write(&bad, b"this is not audio at all, just text pretending").unwrap();
    let store = MemoryStore::new();
    let lib = library(root);

    let scan = scan_library(&store, &lib);
    assert_eq!(
        scan.state,
        ScanState::Completed,
        "one bad file never aborts a scan"
    );
    assert_eq!(scan.progress.added, 1);
    assert_eq!(scan.progress.problems, 1);
    let problems = store.problems(&lib.id);
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path, bad);

    // Replace with a real file: the problem clears.
    write_small_wav(&bad);
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.added, 1);
    assert!(store.problems(&lib.id).is_empty());
}

#[test]
fn untagged_file_is_named_after_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let p = root.join("loose/My Song.wav");
    write_small_wav(&p);
    let store = MemoryStore::new();
    let lib = library(root);
    scan_library(&store, &lib);
    let t = store.tracks(&lib.id)[0].record.clone();
    assert_eq!(t.tags.title, None);
    assert_eq!(t.display_title(), "My Song");
    assert!(t.tags.artists.is_empty());
    assert_eq!(t.sort.title, "my song");
    assert_eq!(t.properties.format, jewelcase_core::Format::Wav);
    assert_eq!(t.properties.channels, Some(2));
    assert_eq!(t.properties.sample_rate, Some(44_100));
}

#[test]
fn excludes_are_never_indexed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    make_album(root, "A", "X", 1);
    make_album(root, "backup", "X", 1);
    let store = MemoryStore::new();
    let mut lib = library(root);
    lib.excludes = vec!["backup".into(), "backup/**".into()];
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.files_seen, 1);
    assert_eq!(paths_of(&store, &lib).len(), 1);
}

#[test]
fn folder_scope_reconciles_only_inside_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    make_album(root, "A", "X", 1);
    make_album(root, "B", "Y", 1);
    let store = MemoryStore::new();
    let lib = library(root);
    scan_library(&store, &lib);

    fs::remove_file(root.join("B/Y/01 - Track 1.wav")).unwrap();
    // Scan only A: B's missing file must not be noticed.
    let scan = scan_once(
        &store,
        &lib,
        Trigger::Manual,
        vec![Scope::folder(root, root.join("A"), Depth::Subtree)],
    );
    assert_eq!(scan.progress.missing, 0);
    assert!(store.tracks(&lib.id).iter().all(|t| !t.missing));
    // Now scan B.
    let scan = scan_once(
        &store,
        &lib,
        Trigger::Manual,
        vec![Scope::folder(root, root.join("B"), Depth::Subtree)],
    );
    assert_eq!(scan.progress.missing, 1);
}

#[test]
fn queue_runs_resumes_and_coalesces() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    make_album(root, "A", "X", 2);
    make_album(root, "B", "Y", 2);
    make_album(root, "C", "Z", 2);
    let store = shared(MemoryStore::new());
    let lib = library(root);

    // A scan interrupted by a crash after finishing directory "A/X" of root
    // scope 0 is left Running with a cursor. Starting the scanner resumes it.
    let interrupted = Scan {
        id: 0,
        library: lib.id.clone(),
        trigger: Trigger::Initial,
        scopes: vec![Scope::root(root)],
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
    let scans = store.scans(&lib.id);
    let resumed = scans.iter().find(|s| s.id == interrupted_row.id).unwrap();
    assert_eq!(resumed.state, ScanState::Completed);
    // Only B and C were walked (A/X and its ancestors were "already done").
    assert_eq!(resumed.progress.files_seen, 4, "{:?}", resumed.progress);

    // Coalescing: a whole-library request subsumes folder requests queued
    // behind it, and an identical request returns the same scan.
    let a = scanner.request(
        Trigger::Manual,
        vec![Scope::folder(root, root.join("A"), Depth::Subtree)],
    );
    let b = scanner.request(
        Trigger::Manual,
        vec![Scope::folder(root, root.join("A/X"), Depth::Directory)],
    );
    let all = scanner.scan_library(Trigger::Scheduled);
    let again = scanner.scan_library(Trigger::Manual);
    assert_eq!(a, b, "a covered scope joins the existing scan");
    assert_eq!(all, again);
    scanner.wait_idle();
    let final_scans = store.scans(&lib.id);
    let a_scan = final_scans.iter().find(|s| s.id == a).unwrap();
    assert!(matches!(
        a_scan.state,
        ScanState::Completed | ScanState::Cancelled
    ));
    assert_eq!(
        final_scans.iter().find(|s| s.id == all).unwrap().state,
        ScanState::Completed
    );
    assert_eq!(paths_of(&store, &lib).len(), 6);
    scanner.shutdown();
}
