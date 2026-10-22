//! The SQLite store against the scanner's pipeline suite, plus the
//! derived-data rules the schema assigns to the scanner.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jewelcase_core::Format;
use jewelcase_scanner::testing::{self, InspectStore, InspectTrack};
use jewelcase_scanner::*;
use lofty::tag::ItemKey;
use rusqlite::{OptionalExtension, Result};

use super::Database;
use super::libraries;
use super::store::SqliteStore;
use super::store::names::parse;
use super::testing::Image;
use super::waveform;

/// The store [`library_store`] makes for the library with its one root at `root`.
fn make(root: &Path) -> SqliteStore {
    library_store(&testing::library(root))
}

/// The suite's [`testing::Factory`]: a store holding `lib`, with id 1 and every one of its roots,
/// whose database lives beside its first root.
fn library_store(lib: &LibraryConfig) -> SqliteStore {
    let dir = lib.roots[0].parent().unwrap().join("state");
    std::fs::create_dir_all(&dir).unwrap();
    let db = Arc::new(Database::open(&dir.join("jewelcase.db")).unwrap());
    let roots = lib.roots.clone();
    db.write_blocking(move |tx| {
        let exists: Option<i64> =
            tx.query_row("SELECT id FROM libraries WHERE id = 1", [], |r| r.get(0)).optional()?;
        if exists.is_none() {
            let roots: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
            libraries::create(tx, Some(1), "Test", &roots, &[])?;
        }
        Ok(())
    })
    .unwrap();
    SqliteStore::new(db).unwrap()
}

fn format_of(codec: &str, container: &str) -> Format {
    match (codec, container) {
        ("flac", _) => Format::Flac,
        ("alac", _) => Format::Alac,
        ("mp3", _) => Format::Mp3,
        ("aac", _) => Format::Aac,
        ("vorbis", _) => Format::Vorbis,
        ("opus", _) => Format::Opus,
        (_, "aiff") => Format::Aiff,
        _ => Format::Wav,
    }
}

impl InspectStore for SqliteStore {
    fn inspect_tracks(&self, lib: LibraryId) -> Vec<InspectTrack> {
        self.db().read_blocking(move |conn| {
        let roots: std::collections::HashMap<i64, PathBuf> = libraries::roots(conn, lib)
            .unwrap()
            .into_iter()
            .map(|r| (r.id, r.path))
            .collect();
        let mut stmt = conn
            .prepare(
                "SELECT t.id, t.root_id, t.path, t.missing_since IS NOT NULL, t.title, t.album_title, t.sort_key, \
                 t.artist_sort_key, t.codec, t.container, t.channels, t.sample_rate_hz, t.analyzer_version > 0, \
                 t.loudness_lufs IS NOT NULL, \
                 (SELECT image_id FROM albums a WHERE a.id = t.album_id) IS NOT NULL, \
                 EXISTS (SELECT 1 FROM track_artists ta JOIN artists ar ON ar.id = ta.artist_id \
                         WHERE ta.track_id = t.id AND ar.biography IS NOT NULL), \
                 t.lyrics_kind <> 'none' \
                 FROM tracks t WHERE t.library_id = ?1 ORDER BY t.path",
            )
            .unwrap();
        let rows = stmt
            .query_map([lib], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, bool>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Vec<u8>>(6)?,
                    r.get::<_, Vec<u8>>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, i64>(10)?,
                    r.get::<_, i64>(11)?,
                    r.get::<_, bool>(12)?,
                    r.get::<_, bool>(13)?,
                    r.get::<_, bool>(14)?,
                    r.get::<_, bool>(15)?,
                    r.get::<_, bool>(16)?,
                ))
            })
            .unwrap();
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                root_id,
                rel,
                missing,
                title,
                album,
                sort_key,
                artist_sort_key,
                codec,
                container,
                channels,
                rate,
                analyzed,
                measured,
                art,
                bio,
                lyr,
            ) = row.unwrap();
            let artists: Vec<String> = {
                let mut s = conn
                    .prepare(
                        "SELECT ar.name FROM track_artists ta JOIN artists ar ON ar.id = ta.artist_id \
                         WHERE ta.track_id = ?1 AND ar.name IS NOT NULL ORDER BY ta.position",
                    )
                    .unwrap();
                s.query_map([id], |r| r.get(0))
                    .unwrap()
                    .collect::<std::result::Result<_, _>>()
                    .unwrap()
            };
            let album_artists: Vec<String> = {
                let mut s = conn
                    .prepare(
                        "SELECT ar.name FROM tracks t JOIN album_artists aa ON aa.album_id = t.album_id \
                         JOIN artists ar ON ar.id = aa.artist_id \
                         WHERE t.id = ?1 AND ar.name IS NOT NULL ORDER BY aa.position",
                    )
                    .unwrap();
                s.query_map([id], |r| r.get(0))
                    .unwrap()
                    .collect::<std::result::Result<_, _>>()
                    .unwrap()
            };
            out.push(InspectTrack {
                id,
                path: roots[&root_id].join(&rel),
                missing,
                title,
                artists,
                album,
                album_artists,
                has_artwork: art,
                has_biography: bio,
                has_lyrics: lyr,
                sort_title: String::from_utf8(sort_key).unwrap(),
                sort_artist: String::from_utf8(artist_sort_key).unwrap(),
                format: format_of(&codec, &container),
                channels: if channels == 0 {
                    None
                } else {
                    Some(channels as u8)
                },
                sample_rate: if rate == 0 { None } else { Some(rate as u32) },
                analyzed,
                measured,
            });
        }
        Ok(out)
        })
        .unwrap()
    }

    fn inspect_problems(&self, lib: LibraryId) -> Vec<Problem> {
        self.db().read_blocking(move |conn| {
        let roots: std::collections::HashMap<i64, PathBuf> = libraries::roots(conn, lib)
            .unwrap()
            .into_iter()
            .map(|r| (r.id, r.path))
            .collect();
        let mut stmt = conn
            .prepare(
                "SELECT p.root_id, p.path, g.kind, p.detail, p.seen_at FROM scan_problems p \
                 JOIN scan_problem_groups g ON g.id = p.group_id WHERE p.library_id = ?1 ORDER BY p.path",
            )
            .unwrap();
        stmt.query_map([lib], |r| {
            Ok(Problem {
                path: roots[&r.get::<_, i64>(0)?].join(r.get::<_, String>(1)?),
                kind: parse(r, 2)?,
                detail: r.get(3)?,
                seen_at: r.get(4)?,
            })
        })
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        })
        .unwrap()
    }

    fn inspect_placeholder(&self, lib: LibraryId, hash: &[u8]) -> Option<Vec<u8>> {
        let hash = hash.to_vec();
        self.db()
            .read_blocking(move |conn| {
                conn.query_row(
                    "SELECT placeholder FROM images WHERE library_id = ?1 AND hash = ?2",
                    rusqlite::params![lib, hash],
                    |r| r.get::<_, Option<Vec<u8>>>(0),
                )
                .optional()
            })
            .unwrap()
            .flatten()
    }

    fn inspect_scans(&self, lib: LibraryId) -> Vec<Scan> {
        self.scans(lib).unwrap()
    }

    fn feed_head(&self, lib: LibraryId) -> i64 {
        self.db()
            .read_blocking(move |conn| {
                conn.query_row(
                    "SELECT ifnull(max(seq), 0) FROM library_changes WHERE library_id = ?1",
                    [lib],
                    |r| r.get(0),
                )
            })
            .unwrap()
    }

    fn feed_tracks_after(&self, lib: LibraryId, head: i64) -> Vec<TrackId> {
        self.db()
            .read_blocking(move |conn| {
                conn.prepare(
                    "SELECT entity_id FROM library_changes \
                     WHERE library_id = ?1 AND entity_type = 'track' AND seq > ?2 ORDER BY seq",
                )?
                .query_map([lib, head], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>>>()
            })
            .unwrap()
            .into_iter()
            .map(|id| TrackId::try_from(id).unwrap())
            .collect()
    }
}

mod suite {
    jewelcase_scanner::store_suite!(super::library_store);
}

mod artist_sort;
mod derived;
mod failures;
mod labels;
mod lyrics;
mod marks;
mod primary;
mod purge;
mod reanalysis;
mod recompute;
mod scans;
mod sidecars;
mod totals;
mod track_totals;

// ───────────────────────────── Derived rules ─────────────────────────────

fn count(store: &SqliteStore, sql: &'static str) -> i64 {
    store.db().read_blocking(move |conn| conn.query_row(sql, [], |r| r.get(0))).unwrap()
}

fn scalar<T>(store: &SqliteStore, sql: &'static str) -> Option<T>
where
    T: rusqlite::types::FromSql + Send + 'static,
{
    store.db().read_blocking(move |conn| conn.query_row(sql, [], |r| r.get(0)).optional()).unwrap()
}

fn tagged(root: &Path, rel: &str, items: &[(ItemKey, &str)]) {
    let p = root.join(rel);
    testing::write_small_wav(&p);
    testing::tag_wav_with(&p, items);
}

#[test]
fn a_placeholder_is_an_album_change() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=red:s=8x8"])
        .args(["-frames:v", "1"])
        .arg(root.join("A/X/cover.png"))
        .status()
        .unwrap();
    assert!(status.success());
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    let album_seq = |store: &SqliteStore| {
        scalar::<i64>(store, "SELECT seq FROM library_changes WHERE entity_type = 'album'").unwrap()
    };
    let (hash, _) = store.next_without_placeholder(lib.id, 1).unwrap().remove(0);

    // One that could not be made changes nothing clients see.
    let before = album_seq(&store);
    store.store_placeholder(lib.id, &hash, Vec::new()).unwrap();
    assert_eq!(album_seq(&store), before);
    store.store_placeholder(lib.id, &hash, vec![1, 2, 3]).unwrap();
    assert!(album_seq(&store) > before);
}

#[test]
fn album_split_across_folders_is_one_album() -> Result<()> {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(
        &root,
        "disc1/01.wav",
        &[
            (ItemKey::TrackTitle, "A"),
            (ItemKey::TrackArtist, "Band"),
            (ItemKey::AlbumTitle, "Live"),
            (ItemKey::DiscNumber, "1"),
            (ItemKey::TrackTotal, "10"),
        ],
    );
    tagged(
        &root,
        "disc2/01.wav",
        &[
            (ItemKey::TrackTitle, "B"),
            (ItemKey::TrackArtist, "Band"),
            (ItemKey::AlbumTitle, "live"),
            (ItemKey::DiscNumber, "2"),
            (ItemKey::TrackTotal, "8"),
        ],
    );
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM albums"),
        1,
        "case-folded title, same artists: one album"
    );
    assert_eq!(scalar::<i64>(&store, "SELECT track_count FROM albums"), Some(2));
    assert_eq!(scalar::<i64>(&store, "SELECT disc_count FROM albums"), Some(2));
    assert_eq!(
        scalar::<i64>(&store, "SELECT track_total FROM albums"),
        Some(18),
        "the sum of its discs' totals"
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM album_discs"), 2);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM album_artists"), 1);
    assert_eq!(
        scalar::<i64>(&store, "SELECT album_count FROM artists WHERE name_key = 'band'"),
        Some(1)
    );
    assert_eq!(scalar::<i64>(&store, "SELECT track_count FROM libraries WHERE id = 1"), Some(2));
    Ok(())
}

#[test]
fn most_used_spelling_wins_and_ties_break_by_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(
        &root,
        "a/01.wav",
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "nirvana"),
            (ItemKey::AlbumTitle, "X"),
        ],
    );
    tagged(
        &root,
        "a/02.wav",
        &[
            (ItemKey::TrackTitle, "2"),
            (ItemKey::TrackArtist, "Nirvana"),
            (ItemKey::AlbumTitle, "X"),
        ],
    );
    tagged(
        &root,
        "a/03.wav",
        &[
            (ItemKey::TrackTitle, "3"),
            (ItemKey::TrackArtist, "Nirvana"),
            (ItemKey::AlbumTitle, "X"),
        ],
    );
    // A tie elsewhere: one each, "Beck" sorts before "beck" by bytes.
    tagged(
        &root,
        "b/01.wav",
        &[(ItemKey::TrackTitle, "1"), (ItemKey::TrackArtist, "beck"), (ItemKey::AlbumTitle, "Y")],
    );
    tagged(
        &root,
        "b/02.wav",
        &[(ItemKey::TrackTitle, "2"), (ItemKey::TrackArtist, "Beck"), (ItemKey::AlbumTitle, "Y")],
    );
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM artists"),
        2,
        "spellings fold to one artist each"
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT name FROM artists WHERE name_key = 'nirvana'"),
        Some("Nirvana".into())
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT name FROM artists WHERE name_key = 'beck'"),
        Some("Beck".into())
    );
    assert_eq!(
        scalar::<i64>(&store, "SELECT track_count FROM artists WHERE name_key = 'nirvana'"),
        Some(3)
    );
}

#[test]
fn retag_moves_track_and_removes_emptied_album_and_artist() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(
        &root,
        "x/01.wav",
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "Old"),
            (ItemKey::AlbumTitle, "First"),
            (ItemKey::Genre, "Rock"),
        ],
    );
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM artists WHERE name = 'Old'"), 1);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM tags"), 1);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let p = root.join("x/01.wav");
    testing::tag_wav_with(
        &p,
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "New"),
            (ItemKey::AlbumTitle, "Second"),
            (ItemKey::Genre, "Jazz"),
        ],
    );
    std::fs::File::open(&p)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    let scan = testing::scan_library(&store, &lib);
    assert_eq!(scan.progress.updated, 1);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM tracks"), 1);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM artists"), 1, "Old is gone");
    assert_eq!(scalar::<String>(&store, "SELECT name FROM artists"), Some("New".into()));
    assert_eq!(count(&store, "SELECT COUNT(*) FROM albums"), 1);
    assert_eq!(scalar::<String>(&store, "SELECT title FROM albums"), Some("Second".into()));
    assert_eq!(scalar::<String>(&store, "SELECT name FROM tags"), Some("Jazz".into()));
    // The feed carries tombstones for what vanished and one row per entity.
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM library_changes WHERE entity_type = 'artist' AND op = 'delete'"
        ),
        1
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM library_changes WHERE entity_type = 'track'"),
        1
    );
    assert_eq!(scalar::<i64>(&store, "SELECT artist_count FROM libraries WHERE id = 1"), Some(1));
}

#[test]
fn untagged_files_share_the_unknown_artist_and_album() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::write_small_wav(&root.join("a/one.wav"));
    testing::write_small_wav(&root.join("b/two.wav"));
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM artists"), 1);
    assert_eq!(
        scalar::<Option<String>>(&store, "SELECT name FROM artists"),
        Some(None),
        "the unknown artist has no name"
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM albums"), 1);
    assert_eq!(scalar::<Option<String>>(&store, "SELECT title FROM albums"), Some(None));
    assert_eq!(scalar::<i64>(&store, "SELECT track_count FROM albums"), Some(2));
    assert_eq!(
        scalar::<String>(&store, "SELECT title FROM tracks WHERE path = 'a/one.wav'"),
        Some("one".into())
    );
}

#[test]
fn unreadable_settings_fail_loudly_rather_than_read_as_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    // A valid JSON array, as the schema checks, but not of what it should hold.
    store
        .db()
        .write_blocking(|tx| tx.execute("UPDATE libraries SET excludes = '[1]'", []))
        .unwrap();
    assert!(store.db().read_blocking(libraries::all).is_err());
}

#[test]
fn a_scan_that_cannot_be_recorded_is_not_recorded_empty() {
    use std::os::unix::ffi::OsStrExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    // A folder name that is not UTF-8, as Linux allows.
    let folder = root.join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
    let scopes = [Scope::folder(&root, folder, Depth::Directory)];
    assert!(store.create_scan(lib.id, Trigger::Watch, &scopes).is_err());
    assert!(store.scans(1).unwrap().is_empty());
}

#[test]
fn compilations_and_album_types() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(
        &root,
        "c/01.wav",
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "A"),
            (ItemKey::AlbumTitle, "Hits"),
            (ItemKey::AlbumArtist, "Various Artists"),
            (ItemKey::FlagCompilation, "1"),
        ],
    );
    tagged(
        &root,
        "c/02.wav",
        &[
            (ItemKey::TrackTitle, "2"),
            (ItemKey::TrackArtist, "B"),
            (ItemKey::AlbumTitle, "Hits"),
            (ItemKey::AlbumArtist, "Various Artists"),
            (ItemKey::FlagCompilation, "1"),
        ],
    );
    tagged(
        &root,
        "e/01.wav",
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "C"),
            (ItemKey::AlbumTitle, "Short"),
            (ItemKey::TrackTotal, "5"),
        ],
    );
    tagged(
        &root,
        "s/01.wav",
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "D"),
            (ItemKey::AlbumTitle, "One"),
            (ItemKey::TrackTotal, "2"),
            (ItemKey::MusicBrainzReleaseType, "album"),
        ],
    );
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM albums WHERE title = 'Hits'"),
        1,
        "one compilation, not one per artist"
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT type FROM albums WHERE title = 'Hits'"),
        Some("compilation".into())
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT type FROM albums WHERE title = 'Short'"),
        Some("ep".into())
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT type FROM albums WHERE title = 'One'"),
        Some("album".into()),
        "the tag beats the size"
    );
    // A and B appear on Hits but do not own it (`requirements/artists.md` §2).
    assert_eq!(
        scalar::<i64>(&store, "SELECT appearance_count FROM artists WHERE name = 'A'"),
        Some(1)
    );
    assert_eq!(scalar::<i64>(&store, "SELECT album_count FROM artists WHERE name = 'A'"), Some(0));
    assert_eq!(
        scalar::<i64>(&store, "SELECT album_count FROM artists WHERE name = 'Various Artists'"),
        Some(1)
    );
}

#[test]
fn lyrics_and_artwork_land_in_their_tables() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    tagged(
        &root,
        "a/01.wav",
        &[(ItemKey::TrackTitle, "1"), (ItemKey::TrackArtist, "A"), (ItemKey::AlbumTitle, "X")],
    );
    std::fs::write(root.join("a/01.lrc"), "[00:01.00]first\n[00:02.50]second").unwrap();
    // Not square, so a width and height stored the wrong way round show.
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=red:s=2x1"])
        .args(["-frames:v", "1"])
        .arg(root.join("a/cover.png"))
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::copy(root.join("a/cover.png"), root.join("a/artist.png")).unwrap();
    std::fs::write(root.join("a/artist.txt"), "A biography.").unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(scalar::<String>(&store, "SELECT lyrics_kind FROM tracks"), Some("synced".into()));
    let synced: String = scalar(&store, "SELECT synced FROM track_lyrics").unwrap();
    let lines: Vec<serde_json::Value> = serde_json::from_str(&synced).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["startMs"], 2500);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM images"),
        1,
        "cover and artist image are the same bytes: one row"
    );
    assert_eq!(
        scalar::<String>(&store, "SELECT width || 'x' || height FROM images"),
        Some("2x1".into())
    );
    assert!(scalar::<i64>(&store, "SELECT image_id FROM albums").is_some());
    assert!(scalar::<i64>(&store, "SELECT image_id FROM artists WHERE name = 'A'").is_some());
    assert_eq!(
        scalar::<String>(&store, "SELECT biography FROM artists WHERE name = 'A'"),
        Some("A biography.".into())
    );
}

#[test]
fn problems_group_by_kind_and_clear() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::write(root.join("a/one.flac"), b"garbage one").unwrap();
    std::fs::write(root.join("a/two.flac"), b"garbage two").unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM scan_problem_groups"), 1, "one systemic issue");
    assert_eq!(scalar::<i64>(&store, "SELECT count FROM scan_problem_groups"), Some(2));
    testing::write_small_wav(&root.join("a/one.flac"));
    testing::scan_library(&store, &lib);
    assert_eq!(scalar::<i64>(&store, "SELECT count FROM scan_problem_groups"), Some(1));
    std::fs::remove_file(root.join("a/two.flac")).unwrap();
    testing::scan_library(&store, &lib);
    // A deleted file takes its problem with it, and the emptied group goes.
    assert_eq!(count(&store, "SELECT COUNT(*) FROM scan_problem_groups"), 0);
}

#[test]
fn analysis_results_reach_tracks_albums_and_waveforms() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("skipping: ffmpeg not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::write_sine_wav(&root.join("a/01.wav"), 1000.0, 0.5, 1.0);
    testing::tag_wav(&root.join("a/01.wav"), "1", "A", "X");
    let store = Arc::new(make(&root));
    let lib = testing::library(&root);
    testing::scan_library(store.as_ref(), &lib);
    let ffmpeg = jewelcase_ffmpeg::Ffmpeg::new(jewelcase_ffmpeg::Config::default());
    let analyzer = jewelcase_scanner::Analyzer::new(ffmpeg, store.clone());
    let report = analyzer.run_once(lib.id, 10);
    assert_eq!(report.done, 1);
    assert_eq!(analyzer.run_once(lib.id, 10).done, 0, "nothing left");
    let lufs: f64 = scalar(&store, "SELECT loudness_lufs FROM tracks").unwrap();
    assert!((lufs - -6.7).abs() < 1.0, "{lufs}");
    assert_eq!(
        scalar::<i64>(&store, "SELECT analyzer_version FROM tracks"),
        Some(jewelcase_scanner::ANALYZER_VERSION as i64)
    );
    let album_lufs: f64 = scalar(&store, "SELECT loudness_lufs FROM albums").unwrap();
    assert!((album_lufs - lufs).abs() < 0.01, "a one-track album is as loud as its track");
    let blob: Vec<u8> = scalar(&store, "SELECT data FROM track_waveforms").unwrap();
    assert_eq!(waveform::decode(&blob).unwrap().len(), 512);
}

#[test]
fn another_librarys_track_is_left_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    let other = tmp.path().join("other");
    store
        .db()
        .write_blocking(move |tx| {
            libraries::create(tx, Some(2), "Other", &[&other], &[])?;
            tx.execute_batch(
                "UPDATE tracks SET missing_since = 5;
                 INSERT INTO track_waveforms (library_id, track_id, data)
                 SELECT library_id, id, x'00' FROM tracks;",
            )
        })
        .unwrap();
    store.refresh_roots().unwrap();
    let track: TrackId = scalar(&store, "SELECT id FROM tracks").unwrap();

    let other: LibraryId = 2;
    let batch = Batch { returned: vec![track], changed_unread: vec![track], ..Batch::default() };
    store.apply(other, batch).unwrap();
    // No RMS levels: the path that deletes a waveform. The file is as picked, so only the library
    // stops the write.
    let result = AnalysisResult { integrated_lufs: Some(-9.0), ..AnalysisResult::empty(1) };
    let stamp = |sql| scalar::<i64>(&store, sql).unwrap() as u64;
    let (size, mtime_ms) =
        (stamp("SELECT file_size FROM tracks"), stamp("SELECT file_mtime FROM tracks"));
    store.store_analysis(other, PickedTrack { id: track, size, mtime_ms }, result).unwrap();
    // A placeholder made in library 2 for content only library 1 has an image of.
    store.db().write_blocking(|tx| Image::new(1, 1, "A/X/cover.png").insert(tx)).unwrap();
    let hash = 1_i64.to_be_bytes();
    store.store_placeholder(other, &hash, vec![1, 2, 3]).unwrap();

    let row = "SELECT missing_since, analyzer_version FROM tracks";
    let unchanged = store
        .db()
        .read_blocking(move |conn| {
            conn.query_row(row, [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
        })
        .unwrap();
    assert_eq!(unchanged, (5, 0));
    assert_eq!(count(&store, "SELECT COUNT(*) FROM track_waveforms"), 1);
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM images WHERE id = 1 AND placeholder IS NULL"),
        1
    );
    assert_eq!(store.feed_head(other), 0);
}

/// A batch applied for another library marks none of this one's tracks missing, whether it names
/// a track or a folder holding one.
#[test]
fn another_librarys_batch_marks_nothing_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    testing::make_album(&root, "A", "X", 1);
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    let other = tmp.path().join("other");
    store
        .db()
        .write_blocking(move |tx| libraries::create(tx, Some(2), "Other", &[&other], &[]))
        .unwrap();
    store.refresh_roots().unwrap();
    let (track, head): (TrackId, i64) =
        (scalar(&store, "SELECT id FROM tracks").unwrap(), store.feed_head(lib.id));

    let batch = Batch { missing: vec![track], vanished: vec![root.join("A")], ..Batch::default() };
    assert_eq!(store.apply(2, batch).unwrap(), 0);
    assert!(store.inspect_tracks(lib.id).iter().all(|t| !t.missing));
    assert_eq!((store.feed_head(lib.id), store.feed_head(2)), (head, 0));
}

#[test]
fn an_upsert_naming_another_librarys_track_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    // A track with an artist, an album artist, and a genre.
    store.apply(lib.id, Batch { upserts: vec![record(&root, 0)], ..Batch::default() }).unwrap();
    let other_root = tmp.path().join("other");
    let created = other_root.clone();
    store
        .db()
        .write_blocking(move |tx| libraries::create(tx, Some(2), "Other", &[&created], &[]))
        .unwrap();
    store.refresh_roots().unwrap();
    let (before, head) = (store.inspect_tracks(lib.id), store.feed_head(lib.id));

    // A record for a file under library 2's root that carries library 1's track ID.
    let other: LibraryId = 2;
    let mut batch = Batch::default();
    batch.upserts.push(TrackRecord { id: Some(before[0].id), ..record(&other_root, 0) });
    store.apply(other, batch).unwrap();

    assert_eq!(store.inspect_tracks(lib.id), before);
    assert_eq!(store.feed_head(lib.id), head);
    assert!(store.inspect_tracks(other).is_empty());
    let made = "SELECT (SELECT COUNT(*) FROM albums WHERE library_id = 2) \
                + (SELECT COUNT(*) FROM artists WHERE library_id = 2)";
    assert_eq!(count(&store, made), 0, "nothing made for it is left behind");
    let named = "SELECT COUNT(*) FROM library_changes c WHERE c.library_id = 2 AND ( \
                 (c.entity_type = 'artist' AND c.entity_id IN (SELECT id FROM artists WHERE library_id = 1)) \
                 OR (c.entity_type = 'tag' AND c.entity_id IN (SELECT id FROM tags WHERE library_id = 1)) \
                 OR (c.entity_type = 'album' AND c.entity_id IN (SELECT id FROM albums WHERE library_id = 1)))";
    assert_eq!(count(&store, named), 0, "library 2's feed names nothing of library 1's");
}

/// Synthetic record `i` under `root`, as the scanner makes for a new file: ten tracks to an
/// album, ten albums to an artist, and three genres.
fn record(root: &Path, i: usize) -> TrackRecord {
    let artist = format!("Artist {}", i / 100);
    let album = format!("Album {}", i / 10);
    let path = root.join(format!("{artist}/{album}/{:02} - Track {i}.flac", i % 10 + 1));
    TrackRecord {
        id: None,
        root: root.to_path_buf(),
        path,
        size: 30_000_000 + i as u64,
        mtime_ms: 1_700_000_000_000 + i as u64,
        tags: TagSet {
            title: Some(format!("Track {i}")),
            artists: vec![artist.clone()],
            album: Some(album),
            album_artists: vec![artist],
            track_number: Some((i % 10 + 1) as u32),
            track_total: Some(10),
            genres: vec![format!("Genre {}", i % 3)],
            release_date: PartialDate::parse("1999"),
            ..Default::default()
        },
        properties: AudioProperties {
            format: Format::Flac,
            duration_ms: 240_000,
            sample_rate: Some(44_100),
            bit_depth: Some(16),
            channels: Some(2),
            bitrate_kbps: Some(900),
        },
        artwork: None,
        artist_image: None,
        artist_biography: None,
        lyrics_sidecar: None,
        sidecar_stamp: 0,
        sort: SortKeys {
            title: format!("track {i}"),
            artist: format!("artist {}", i / 100),
            album: format!("album {}", i / 10),
            album_artist: format!("artist {}", i / 100),
        },
    }
}

/// The cold-scan budget, store side only (`design/scanning.md` §15,
/// `requirements/performance.md` §5): does persisting a large library and
/// reading what is indexed in every folder fit inside an hour? Run with
/// `cargo test --release -p jewelcase-server benchmark_cold_scan -- --ignored --nocapture`.
/// `BUDGET_TRACKS` sets the library size (default 50,000); each time is also printed scaled
/// to 500,000 tracks.
#[test]
#[ignore = "benchmark"]
fn benchmark_cold_scan_store_only() {
    use std::time::Instant;
    let tracks: usize =
        std::env::var("BUDGET_TRACKS").ok().and_then(|v| v.parse().ok()).unwrap_or(50_000);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);

    let synth = |i: usize| record(&root, i);

    let started = Instant::now();
    let batch_size = 250;
    let mut i = 0;
    while i < tracks {
        let mut batch = Batch::default();
        for j in i..(i + batch_size).min(tracks) {
            batch.upserts.push(synth(j));
        }
        store.apply(lib.id, batch).unwrap();
        i += batch_size;
    }
    let insert = started.elapsed();

    // An unchanged rescan reads what is indexed in each folder, root and artists included, and
    // writes nothing.
    let started = Instant::now();
    let mut folders = vec![root.clone()];
    folders.extend(
        (0..tracks).step_by(10).filter_map(|i| synth(i).path.parent().map(Path::to_path_buf)),
    );
    folders.extend((0..tracks).step_by(100).map(|i| root.join(format!("Artist {}", i / 100))));
    let read: usize =
        folders.iter().map(|f| store.indexed_in(lib.id, f).unwrap().files.len()).sum();
    let reads = started.elapsed();
    assert_eq!(read, tracks);

    let per_500k = |d: std::time::Duration| d.as_secs_f64() * 500_000.0 / tracks as f64;
    eprintln!(
        "{tracks} tracks: insert {insert:.2?} ({:.0}s per 500k), unchanged rescan's {} folder reads {reads:.2?} ({:.1}s per 500k)",
        per_500k(insert),
        folders.len(),
        per_500k(reads)
    );
}
