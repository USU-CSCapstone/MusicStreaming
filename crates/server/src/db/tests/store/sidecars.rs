//! Album art and artist images and biographies, resolved from the tracks each scan reads
//! (`requirements/scanning.md` §3, `design/database.md` §3).

use std::fs;
use std::path::Path;

use jewelcase_scanner::testing::{self, ItemKey, PNG};
use jewelcase_scanner::*;

use super::recompute::check;
use super::{count, libraries, make, scalar, tagged};
use crate::db::store::SqliteStore;

/// Writes a small WAV at `rel` under `root`, tagged with `items` and holding `art` as its front
/// cover.
fn with_art(root: &Path, rel: &str, items: &[(ItemKey, &str)], art: &[u8]) {
    let path = root.join(rel);
    testing::write_small_wav(&path);
    testing::tag_wav_with_art(&path, items, art);
}

/// A PNG unlike any other `variant` gives, so each is its own image.
fn png(variant: &str) -> Vec<u8> {
    [PNG, variant.as_bytes()].concat()
}

/// Scans `lib` with `store`, and checks every derived value against a recompute.
fn scan(store: &SqliteStore, lib: &LibraryConfig, step: &str) {
    testing::scan_library(store, lib);
    check(store, step);
}

/// The path of the image the album titled `title` shows, if any.
fn album_art(store: &SqliteStore, title: &'static str) -> Option<String> {
    store
        .db()
        .read_blocking(move |conn| {
            conn.query_row(
                "SELECT i.path FROM albums a LEFT JOIN images i ON i.id = a.image_id \
                 WHERE a.title = ?1",
                [title],
                |r| r.get(0),
            )
        })
        .unwrap()
}

/// Album art is a track's: embedded art before any `cover.*`, then the lowest disc, then the
/// lowest track number.
#[test]
fn embedded_art_comes_first_then_the_lowest_disc_and_track() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let tags = |disc, track| {
        [
            (ItemKey::TrackArtist, "A"),
            (ItemKey::AlbumTitle, "X"),
            (ItemKey::DiscNumber, disc),
            (ItemKey::TrackNumber, track),
        ]
    };
    with_art(&root, "A/X/1.wav", &tags("2", "1"), &png("disc 2"));
    with_art(&root, "A/X/2.wav", &tags("1", "3"), &png("disc 1"));
    tagged(&root, "A/X/3.wav", &tags("1", "2"));
    fs::write(root.join("A/X/cover.png"), PNG).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    scan(&store, &lib, "the first scan");
    assert_eq!(album_art(&store, "X").as_deref(), Some("A/X/2.wav"));

    // Its embedded art gone, the track offers the cover, and the other embedded art wins.
    tagged(&root, "A/X/2.wav", &tags("1", "3"));
    scan(&store, &lib, "the art removed");
    assert_eq!(album_art(&store, "X").as_deref(), Some("A/X/1.wav"));
}

/// A missing track's art gives way to a present track's, whatever their order otherwise.
#[test]
fn a_missing_tracks_art_gives_way() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let tags = |track| {
        [(ItemKey::TrackArtist, "A"), (ItemKey::AlbumTitle, "X"), (ItemKey::TrackNumber, track)]
    };
    with_art(&root, "A/X/1.wav", &tags("1"), &png("first"));
    tagged(&root, "A/X/2.wav", &tags("2"));
    fs::write(root.join("A/X/cover.png"), PNG).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    scan(&store, &lib, "the first scan");
    assert_eq!(album_art(&store, "X").as_deref(), Some("A/X/1.wav"));

    fs::remove_file(root.join("A/X/1.wav")).unwrap();
    scan(&store, &lib, "the first track gone");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM tracks WHERE missing_since IS NOT NULL"), 1);
    assert_eq!(album_art(&store, "X").as_deref(), Some("A/X/cover.png"));
}

/// A lyrics file rewritten at the same size is read again, since its modified time changed.
#[test]
fn a_lyrics_file_rewritten_at_the_same_size_is_read_again() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let lrc = root.join("A/X/01 - Track 1.lrc");
    fs::write(&lrc, "[00:01.00]aaaa").unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    scan(&store, &lib, "the first scan");

    let at = fs::metadata(&lrc).unwrap().modified().unwrap();
    fs::write(&lrc, "[00:01.00]bbbb").unwrap();
    let file = fs::File::options().write(true).open(&lrc).unwrap();
    file.set_modified(at + std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(testing::scan_library(&store, &lib).progress.updated, 1);
    let synced = scalar::<String>(&store, "SELECT synced FROM track_lyrics");
    assert_eq!(synced.as_deref(), Some(r#"[{"startMs":1000,"text":"bbbb"}]"#));
}

/// A cover replaced beside unchanged tracks replaces the album's art, and one removed clears it.
#[test]
fn a_replaced_or_removed_cover_reaches_the_album() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 2);
    let cover = root.join("A/X/cover.png");
    fs::write(&cover, PNG).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    scan(&store, &lib, "the first scan");
    let shown = "SELECT hex(i.hash) FROM albums a JOIN images i ON i.id = a.image_id";
    let first: String = scalar(&store, shown).unwrap();

    fs::write(&cover, png("a new cover")).unwrap();
    scan(&store, &lib, "the cover replaced");
    let second: String = scalar(&store, shown).unwrap();
    assert_ne!(second, first);
    assert_eq!(count(&store, "SELECT COUNT(DISTINCT image_id) FROM tracks"), 1);

    fs::remove_file(&cover).unwrap();
    scan(&store, &lib, "the cover removed");
    assert_eq!(count(&store, "SELECT COUNT(image_id) FROM albums"), 0);
    assert_eq!(count(&store, "SELECT COUNT(image_id) FROM tracks"), 0);
}

/// An artist's image and biography are those of the lowest-numbered track crediting them as album
/// artist, follow a change to its sidecars, and fall to the next track's when they go.
#[test]
fn an_artists_image_and_biography_follow_its_lowest_track() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let tags = |album, track| {
        [(ItemKey::TrackArtist, "B"), (ItemKey::AlbumTitle, album), (ItemKey::TrackNumber, track)]
    };
    tagged(&root, "B/Y/1.wav", &tags("Y", "2"));
    tagged(&root, "B/Z/1.wav", &tags("Z", "1"));
    let sidecar = |rel: &str, bytes: &[u8]| fs::write(root.join(rel), bytes).unwrap();
    sidecar("B/Y/artist.txt", b"From Y.");
    sidecar("B/Y/artist.png", &png("Y"));
    sidecar("B/Z/artist.txt", b"From Z.");
    sidecar("B/Z/artist.png", &png("Z"));
    let store = make(&root);
    let lib = testing::library(&root);
    let shown =
        "SELECT a.biography || ' ' || i.path FROM artists a JOIN images i ON i.id = a.image_id";
    scan(&store, &lib, "the first scan");
    assert_eq!(scalar::<String>(&store, shown).as_deref(), Some("From Z. B/Z/artist.png"));

    sidecar("B/Z/artist.txt", b"From Z, rewritten.");
    scan(&store, &lib, "the biography rewritten");
    assert_eq!(
        scalar::<String>(&store, shown).as_deref(),
        Some("From Z, rewritten. B/Z/artist.png")
    );

    fs::remove_file(root.join("B/Z/artist.txt")).unwrap();
    fs::remove_file(root.join("B/Z/artist.png")).unwrap();
    scan(&store, &lib, "Z's sidecars removed");
    assert_eq!(scalar::<String>(&store, shown).as_deref(), Some("From Y. B/Y/artist.png"));
}

/// Of the artist images an album artist's albums offer, the earliest track's applies: track 1
/// before track 256, whose path sorts first, and on the same track number a path before the
/// longer path it starts.
#[test]
fn an_artists_image_comes_from_its_albums_earliest_track() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let tags = |album, track| {
        [(ItemKey::TrackArtist, "B"), (ItemKey::AlbumTitle, album), (ItemKey::TrackNumber, track)]
    };
    for (folder, album, track) in
        [("B/0", "R", "256"), ("B/X", "P", "1"), ("B/X/1.wav.d", "Q", "1")]
    {
        tagged(&root, &format!("{folder}/1.wav"), &tags(album, track));
        fs::write(root.join(folder).join("artist.png"), png(album)).unwrap();
    }
    let store = make(&root);
    let lib = testing::library(&root);
    scan(&store, &lib, "the first scan");
    let shown = "SELECT i.path FROM artists a JOIN images i ON i.id = a.image_id";
    assert_eq!(scalar::<String>(&store, shown).as_deref(), Some("B/X/artist.png"));
}

/// Two libraries holding the same cover and biography each keep their own rows, and a cover
/// removed from one leaves the other's album alone.
#[test]
fn sidecars_resolve_within_their_library() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, other) = (tmp.path().join("lib"), tmp.path().join("other"));
    for dir in [&root, &other] {
        testing::make_album(dir, "A", "X", 1);
        fs::write(dir.join("A/X/cover.png"), PNG).unwrap();
        fs::write(dir.join("A/artist.txt"), "A band.").unwrap();
    }
    let store = make(&root);
    let created = other.clone();
    store
        .db()
        .write_blocking(move |tx| libraries::create(tx, Some(2), "Other", &[&created], &[]))
        .unwrap();
    store.refresh_roots().unwrap();
    let (one, two) = (testing::library(&root), testing::library(&other));
    let two = LibraryConfig { id: 2, ..two };
    scan(&store, &one, "library 1's scan");
    scan(&store, &two, "library 2's scan");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM images"), 2, "one per library");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM biographies"), 2, "one per library");
    let crossed = "SELECT \
        (SELECT COUNT(*) FROM tracks t JOIN images i ON i.id = t.image_id \
         WHERE i.library_id <> t.library_id) \
        + (SELECT COUNT(*) FROM tracks t JOIN biographies b ON b.id = t.biography_id \
           WHERE b.library_id <> t.library_id) \
        + (SELECT COUNT(*) FROM albums a JOIN images i ON i.id = a.image_id \
           WHERE i.library_id <> a.library_id)";
    assert_eq!(count(&store, crossed), 0);

    fs::remove_file(root.join("A/X/cover.png")).unwrap();
    scan(&store, &one, "library 1's cover removed");
    let art = "SELECT group_concat(library_id || ':' || (image_id IS NOT NULL)) FROM \
               (SELECT library_id, image_id FROM albums ORDER BY library_id)";
    assert_eq!(scalar::<String>(&store, art).as_deref(), Some("1:0,2:1"));
    let bios = "SELECT COUNT(*) FROM artists WHERE biography = 'A band.'";
    assert_eq!(count(&store, bios), 2);
}
