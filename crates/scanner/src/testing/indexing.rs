//! Scenarios for indexing a library: what a first scan finds, and what a rescan updates or leaves
//! alone (`design/scanning.md` §2).

use std::fs;
use std::time::Duration;

use jewelcase_core::Format;

use super::{
    Factory, InspectStore, InspectTrack, ItemKey, PNG, library, live, make_album, scan_library,
    tag_wav, tag_wav_with, tag_wav_with_field, write_small_wav,
};
use crate::scan::ScanState;
use crate::track::TrackId;

/// A first scan indexes each audio file with its tags, cover, biography and lyrics, skips other
/// files and announces every track in the feed; a second scan of the unchanged library reads no
/// file and leaves the feed alone.
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

    let lib = library(&root);
    let store = make(&lib);

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
    assert!(t1.has_lyrics, "lyrics by stem");
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

/// A retagged file with a newer modification time is updated in place and keeps its track id.
pub fn retag_updates_in_place_and_keeps_identity<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    let lib = library(&root);
    let store = make(&lib);
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

/// A file read before a tag type was added is read again for its tags, though it is unchanged,
/// and then not again; read again with nothing new, it announces nothing
/// (`requirements/tags.md` §2).
pub fn an_unchanged_file_is_read_again_for_a_newer_tag_type<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let path = root.join("A/track.wav");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_small_wav(&path);
    tag_wav_with(&path, &[(ItemKey::TrackTitle, "Song"), (ItemKey::Genre, "Rock")]);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    let genres = |store: &S| live(store, &lib)[0].tags.get("genre").cloned().unwrap_or_default();
    assert_eq!(genres(&store), ["Rock"]);

    // The file's genre changes and its size and mtime stay, as a field read by no type would be.
    let (size, mtime) = {
        let meta = fs::metadata(&path).unwrap();
        (meta.len(), meta.modified().unwrap())
    };
    tag_wav_with(&path, &[(ItemKey::TrackTitle, "Song"), (ItemKey::Genre, "Jazz")]);
    fs::File::open(&path).unwrap().set_modified(mtime).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().len(), size, "the retag keeps the file's size");
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.files_processed, 0, "read with the current types, it is not opened");
    assert_eq!(genres(&store), ["Rock"]);

    store.age_tag_reads(lib.id);
    let head = store.feed_head(lib.id);
    let scan = scan_library(&store, &lib);
    let progress = (scan.progress.files_processed, scan.progress.added, scan.progress.updated);
    assert_eq!(progress, (1, 0, 0), "read for its tags alone");
    assert_eq!(genres(&store), ["Jazz"]);
    assert_eq!(store.feed_tracks_after(lib.id, head), [live(&store, &lib)[0].id]);
    assert_eq!(scan_library(&store, &lib).progress.files_processed, 0, "and once only");

    store.age_tag_reads(lib.id);
    let head = store.feed_head(lib.id);
    assert_eq!(scan_library(&store, &lib).progress.files_processed, 1);
    assert_eq!(store.feed_head(lib.id), head, "tags read as they were change nothing");
}

/// A type a plugin adds is read from files already scanned, by the next scan, for their tags
/// alone and once, from the field the plugin writes (`requirements/tags.md` §2).
pub fn a_type_a_plugin_adds_is_read_from_files_already_scanned<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let path = root.join("A/track.wav");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    write_small_wav(&path);
    tag_wav_with_field(&path, "ENERGY", "High");
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    let energy = |store: &S| live(store, &lib)[0].tags.get("acousticbrainz.energy").cloned();
    assert_eq!(energy(&store), None, "no type reads the field yet");

    store.add_tag_type("acousticbrainz.energy", "ENERGY");
    let head = store.feed_head(lib.id);
    let scan = scan_library(&store, &lib);
    let progress = (scan.progress.files_processed, scan.progress.added, scan.progress.updated);
    assert_eq!(progress, (1, 0, 0), "read for its tags alone");
    assert_eq!(energy(&store), Some(vec!["High".to_owned()]));
    assert_eq!(store.feed_tracks_after(lib.id, head), [live(&store, &lib)[0].id]);
    assert_eq!(scan_library(&store, &lib).progress.files_processed, 0, "and once only");
}

/// A lyrics file written beside a track whose audio did not change, as a plugin does,
/// is picked up by the next scan, and dropped when it goes (`requirements/scanning.md` §7).
pub fn lyrics_file_beside_an_unchanged_track<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 2);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    assert!(live(&store, &lib).iter().all(|t| !t.has_lyrics));

    let lrc = root.join("A/X/01 - Track 1.lrc");
    fs::write(&lrc, "[00:01.00]hello\n").unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.updated, 1, "only the track beside the new file is re-read");
    let with: Vec<InspectTrack> = live(&store, &lib).into_iter().filter(|t| t.has_lyrics).collect();
    assert_eq!(with.len(), 1);
    assert!(with[0].path.ends_with("01 - Track 1.wav"));

    let again = scan_library(&store, &lib);
    assert_eq!(again.progress.updated, 0, "and stays incremental afterwards");

    fs::remove_file(&lrc).unwrap();
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.updated, 1);
    assert!(live(&store, &lib).iter().all(|t| !t.has_lyrics));
}

/// A file with no tags is titled after its file name, has no artists, and still has its audio
/// properties read.
pub fn untagged_file_is_named_after_itself<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let p = root.join("loose/My Song.wav");
    write_small_wav(&p);
    let lib = library(&root);
    let store = make(&lib);
    scan_library(&store, &lib);
    let t = live(&store, &lib)[0].clone();
    assert_eq!(t.title, "My Song", "named after its file");
    assert!(t.artists.is_empty());
    assert_eq!(t.sort_title, "my song");
    assert_eq!(t.format, Format::Wav);
    assert_eq!(t.channels, Some(2));
    assert_eq!(t.sample_rate, Some(44_100));
}

/// Files under an excluded folder are neither seen nor indexed.
pub fn excludes_are_never_indexed<S: InspectStore>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    make_album(&root, "A", "X", 1);
    make_album(&root, "backup", "X", 1);
    let store = make(&library(&root));
    let mut lib = library(&root);
    lib.excludes = vec!["backup".into(), "backup/**".into()];
    let scan = scan_library(&store, &lib);
    assert_eq!(scan.progress.files_seen, 1);
    assert_eq!(live(&store, &lib).len(), 1);
}
