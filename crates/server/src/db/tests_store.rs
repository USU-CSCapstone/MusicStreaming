//! The SQLite store against the scanner's conformance suite, plus the
//! derived-data rules the schema assigns to the scanner.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use jewelcase_core::Format;
use jewelcase_scanner::testing::{self, InspectStore, InspectTrack};
use jewelcase_scanner::*;
use lofty::tag::ItemKey;
use rusqlite::{Connection, OptionalExtension};

use super::libraries;
use super::store::SqliteStore;
use super::{Db, Result};

/// The suite's library has id "1"; its database lives beside the root.
fn make(root: &Path) -> SqliteStore {
    let dir = root.parent().unwrap().join("state");
    let db = Arc::new(Db::open(&dir.join("jewelcase.db")).unwrap());
    {
        let conn = db.writer();
        let exists: Option<i64> = conn
            .query_row("SELECT id FROM libraries WHERE id = 1", [], |r| r.get(0))
            .optional()
            .unwrap();
        if exists.is_none() {
            libraries::create_with_id(&conn, 1, "Test", &[root], &[]).unwrap();
        }
    }
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
    fn inspect_tracks(&self, library: &LibraryId) -> Vec<InspectTrack> {
        let lib: i64 = library.parse().unwrap();
        let conn = self.db().writer();
        let roots: std::collections::HashMap<i64, PathBuf> = libraries::roots(&conn, lib)
            .unwrap()
            .into_iter()
            .map(|r| (r.id, r.path))
            .collect();
        let mut stmt = conn
            .prepare(
                "SELECT t.id, t.root_id, t.path, t.missing_since IS NOT NULL, t.title, t.album_title, t.sort_key, \
                 t.artist_sort_key, t.codec, t.container, t.channels, t.sample_rate_hz, t.analyzed_at IS NOT NULL, \
                 (SELECT image_id FROM albums a WHERE a.id = t.album_id) IS NOT NULL, \
                 EXISTS (SELECT 1 FROM track_artists ta JOIN artists ar ON ar.id = ta.artist_id \
                         WHERE ta.track_id = t.id AND ar.biography IS NOT NULL), \
                 EXISTS (SELECT 1 FROM track_lyrics l WHERE l.track_id = t.id AND l.embedded = 0) \
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
            out.push(InspectTrack {
                id: id as u64,
                path: roots[&root_id].join(&rel),
                missing,
                title,
                artists,
                album,
                has_artwork: art,
                has_biography: bio,
                has_sidecar_lyrics: lyr,
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
            });
        }
        out
    }

    fn inspect_problems(&self, library: &LibraryId) -> Vec<Problem> {
        let lib: i64 = library.parse().unwrap();
        let conn = self.db().writer();
        let roots: std::collections::HashMap<i64, PathBuf> = libraries::roots(&conn, lib)
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
                kind: ProblemKind::parse(&r.get::<_, String>(2)?).unwrap(),
                detail: r.get(3)?,
                seen_at: std::time::UNIX_EPOCH
                    + std::time::Duration::from_millis(r.get::<_, i64>(4)? as u64),
            })
        })
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap()
    }

    fn inspect_scans(&self, library: &LibraryId) -> Vec<Scan> {
        self.scans(library.parse().unwrap()).unwrap()
    }

    fn feed_len(&self, library: &LibraryId) -> usize {
        let lib: i64 = library.parse().unwrap();
        self.db()
            .writer()
            .query_row(
                "SELECT COUNT(*) FROM library_changes WHERE library_id = ?1",
                [lib],
                |r| r.get::<_, i64>(0),
            )
            .unwrap() as usize
    }
}

mod suite {
    jewelcase_scanner::store_suite!(super::make);
}

// ───────────────────────────── Derived rules ─────────────────────────────

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn scalar<T: rusqlite::types::FromSql>(conn: &Connection, sql: &str) -> Option<T> {
    conn.query_row(sql, [], |r| r.get(0)).optional().unwrap()
}

fn tagged(root: &Path, rel: &str, items: &[(ItemKey, &str)]) {
    let p = root.join(rel);
    testing::write_small_wav(&p);
    testing::tag_wav_with(&p, items);
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
    let conn = store.db().writer();
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM albums"),
        1,
        "case-folded title, same artists: one album"
    );
    assert_eq!(
        scalar::<i64>(&conn, "SELECT track_count FROM albums"),
        Some(2)
    );
    assert_eq!(
        scalar::<i64>(&conn, "SELECT disc_count FROM albums"),
        Some(2)
    );
    assert_eq!(
        scalar::<i64>(&conn, "SELECT track_total FROM albums"),
        Some(10),
        "max of per-file totals"
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM album_discs"), 2);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM album_artists"), 1);
    assert_eq!(
        scalar::<i64>(
            &conn,
            "SELECT album_count FROM artists WHERE name_key = 'band'"
        ),
        Some(1)
    );
    assert_eq!(
        scalar::<i64>(&conn, "SELECT track_count FROM libraries WHERE id = 1"),
        Some(2)
    );
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
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "beck"),
            (ItemKey::AlbumTitle, "Y"),
        ],
    );
    tagged(
        &root,
        "b/02.wav",
        &[
            (ItemKey::TrackTitle, "2"),
            (ItemKey::TrackArtist, "Beck"),
            (ItemKey::AlbumTitle, "Y"),
        ],
    );
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    let conn = store.db().writer();
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM artists"),
        2,
        "spellings fold to one artist each"
    );
    assert_eq!(
        scalar::<String>(&conn, "SELECT name FROM artists WHERE name_key = 'nirvana'"),
        Some("Nirvana".into())
    );
    assert_eq!(
        scalar::<String>(&conn, "SELECT name FROM artists WHERE name_key = 'beck'"),
        Some("Beck".into())
    );
    assert_eq!(
        scalar::<i64>(
            &conn,
            "SELECT track_count FROM artists WHERE name_key = 'nirvana'"
        ),
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
    {
        let conn = store.db().writer();
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM artists WHERE name = 'Old'"),
            1
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM tags"), 1);
    }
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
    let conn = store.db().writer();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM tracks"), 1);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM artists"),
        1,
        "Old is gone"
    );
    assert_eq!(
        scalar::<String>(&conn, "SELECT name FROM artists"),
        Some("New".into())
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM albums"), 1);
    assert_eq!(
        scalar::<String>(&conn, "SELECT title FROM albums"),
        Some("Second".into())
    );
    assert_eq!(
        scalar::<String>(&conn, "SELECT name FROM tags"),
        Some("Jazz".into())
    );
    // The feed carries tombstones for what vanished and one row per entity.
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM library_changes WHERE entity_type = 'artist' AND op = 'delete'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM library_changes WHERE entity_type = 'track'"
        ),
        1
    );
    assert_eq!(
        scalar::<i64>(&conn, "SELECT artist_count FROM libraries WHERE id = 1"),
        Some(1)
    );
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
    let conn = store.db().writer();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM artists"), 1);
    assert_eq!(
        scalar::<Option<String>>(&conn, "SELECT name FROM artists"),
        Some(None),
        "the unknown artist has no name"
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM albums"), 1);
    assert_eq!(
        scalar::<Option<String>>(&conn, "SELECT title FROM albums"),
        Some(None)
    );
    assert_eq!(
        scalar::<i64>(&conn, "SELECT track_count FROM albums"),
        Some(2)
    );
    assert_eq!(
        scalar::<String>(&conn, "SELECT title FROM tracks WHERE path = 'a/one.wav'"),
        Some("one".into())
    );
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
    let conn = store.db().writer();
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM albums WHERE title = 'Hits'"),
        1,
        "one compilation, not one per artist"
    );
    assert_eq!(
        scalar::<String>(&conn, "SELECT type FROM albums WHERE title = 'Hits'"),
        Some("compilation".into())
    );
    assert_eq!(
        scalar::<String>(&conn, "SELECT type FROM albums WHERE title = 'Short'"),
        Some("ep".into())
    );
    assert_eq!(
        scalar::<String>(&conn, "SELECT type FROM albums WHERE title = 'One'"),
        Some("album".into()),
        "the tag beats the size"
    );
    // A and B appear on Hits but do not own it (`requirements/artists.md` §2).
    assert_eq!(
        scalar::<i64>(
            &conn,
            "SELECT appearance_count FROM artists WHERE name = 'A'"
        ),
        Some(1)
    );
    assert_eq!(
        scalar::<i64>(&conn, "SELECT album_count FROM artists WHERE name = 'A'"),
        Some(0)
    );
    assert_eq!(
        scalar::<i64>(
            &conn,
            "SELECT album_count FROM artists WHERE name = 'Various Artists'"
        ),
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
        &[
            (ItemKey::TrackTitle, "1"),
            (ItemKey::TrackArtist, "A"),
            (ItemKey::AlbumTitle, "X"),
        ],
    );
    std::fs::write(root.join("a/01.lrc"), "[00:01.00]first\n[00:02.50]second").unwrap();
    std::fs::write(root.join("a/cover.png"), testing::PNG).unwrap();
    std::fs::write(root.join("a/artist.png"), testing::PNG).unwrap();
    std::fs::write(root.join("a/artist.txt"), "A biography.").unwrap();
    let store = make(&root);
    let lib = testing::library(&root);
    testing::scan_library(&store, &lib);
    let conn = store.db().writer();
    assert_eq!(
        scalar::<String>(&conn, "SELECT lyrics_kind FROM tracks"),
        Some("synced".into())
    );
    let synced: String = scalar(&conn, "SELECT synced FROM track_lyrics").unwrap();
    let lines: Vec<serde_json::Value> = serde_json::from_str(&synced).unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["startMs"], 2500);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM images"),
        1,
        "cover and artist image are the same bytes: one row"
    );
    assert_eq!(
        scalar::<String>(&conn, "SELECT format FROM images"),
        Some("png".into())
    );
    assert!(scalar::<i64>(&conn, "SELECT image_id FROM albums").is_some());
    assert!(scalar::<i64>(&conn, "SELECT image_id FROM artists WHERE name = 'A'").is_some());
    assert_eq!(
        scalar::<String>(&conn, "SELECT biography FROM artists WHERE name = 'A'"),
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
    {
        let conn = store.db().writer();
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM scan_problem_groups"),
            1,
            "one systemic issue"
        );
        assert_eq!(
            scalar::<i64>(&conn, "SELECT count FROM scan_problem_groups"),
            Some(2)
        );
    }
    testing::write_small_wav(&root.join("a/one.flac"));
    testing::scan_library(&store, &lib);
    let conn = store.db().writer();
    assert_eq!(
        scalar::<i64>(&conn, "SELECT count FROM scan_problem_groups"),
        Some(1)
    );
    std::fs::remove_file(root.join("a/two.flac")).unwrap();
    drop(conn);
    testing::scan_library(&store, &lib);
    // A vanished problem file is not "cleared" by a scan, and stays until it
    // is: that is a purge-policy question, so here we only check nothing broke.
    let conn = store.db().writer();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM scan_problem_groups"), 1);
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
    let analyzer = jewelcase_scanner::analysis::Analyzer::new(
        ffmpeg,
        store.clone(),
        Arc::new(Governor::new()),
    );
    let report = analyzer.run_once(&lib.id, 10);
    assert_eq!(report.analyzed, 1);
    assert_eq!(analyzer.run_once(&lib.id, 10).analyzed, 0, "nothing left");
    let conn = store.db().writer();
    let lufs: f64 = scalar(&conn, "SELECT loudness_lufs FROM tracks").unwrap();
    assert!((lufs - -6.7).abs() < 1.0, "{lufs}");
    assert_eq!(
        scalar::<i64>(&conn, "SELECT analyzer_version FROM tracks"),
        Some(jewelcase_scanner::analysis::ANALYZER_VERSION as i64)
    );
    let album_lufs: f64 = scalar(&conn, "SELECT loudness_lufs FROM albums").unwrap();
    assert!(
        (album_lufs - lufs).abs() < 0.01,
        "a one-track album is as loud as its track"
    );
    let blob: Vec<u8> = scalar(&conn, "SELECT data FROM track_waveforms").unwrap();
    assert_eq!(Waveform::from_blob(&blob).unwrap().peaks.len(), 512);
}

/// The cold-scan budget, store side only (`design/scanning.md` §15,
/// `requirements/performance.md` §5): does persisting a large library and
/// looking every file up fit inside an hour? Run with
/// `cargo test --release -p jewelcase-server budget -- --ignored --nocapture`.
#[test]
#[ignore]
fn budget_store_only() {
    use std::time::Instant;
    let tracks: usize = std::env::var("BUDGET_TRACKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50_000);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);

    // Synthetic records: 10 tracks per album, 10 albums per artist, 3 genres.
    let synth = |i: usize| -> TrackRecord {
        let artist = format!("Artist {}", i / 100);
        let album = format!("Album {}", i / 10);
        let path = root.join(format!(
            "{artist}/{album}/{:02} - Track {i}.flac",
            i % 10 + 1
        ));
        TrackRecord {
            id: None,
            root: root.clone(),
            path,
            size: 30_000_000 + i as u64,
            mtime_ms: 1_700_000_000_000 + i as u64,
            tags: jewelcase_core::TagSet {
                title: Some(format!("Track {i}")),
                artists: vec![artist.clone()],
                album: Some(album),
                album_artists: vec![artist],
                track_number: Some((i % 10 + 1) as u32),
                track_total: Some(10),
                genres: vec![format!("Genre {}", i % 3)],
                release_date: jewelcase_core::PartialDate::parse("1999"),
                ..Default::default()
            },
            properties: jewelcase_core::AudioProperties {
                format: Format::Flac,
                duration_ms: 240_000,
                sample_rate: Some(44_100),
                bit_depth: Some(16),
                channels: Some(2),
                bitrate_kbps: Some(900),
                file_size: 30_000_000,
            },
            artwork: None,
            artist_image: None,
            artist_biography: None,
            lyrics_sidecar: None,
            sort: SortKeys {
                title: format!("track {i}"),
                artist: format!("artist {}", i / 100),
                album: format!("album {}", i / 10),
                album_artist: format!("artist {}", i / 100),
            },
            last_seen_scan: 1,
        }
    };

    let scan_id = store.create_scan(&Scan {
        id: 0,
        library: lib.id.clone(),
        trigger: Trigger::Initial,
        scopes: vec![Scope::root(&root)],
        state: ScanState::Running,
        started_at: None,
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: None,
    });

    let started = Instant::now();
    let batch_size = 250;
    let mut i = 0;
    while i < tracks {
        let mut batch = Batch::new(scan_id);
        for j in i..(i + batch_size).min(tracks) {
            batch.upserts.push(synth(j));
        }
        store.apply(&lib.id, batch).unwrap();
        i += batch_size;
    }
    let insert = started.elapsed();

    let started = Instant::now();
    let mut hits = 0;
    for j in 0..tracks {
        if store.lookup(&lib.id, &synth(j).path).is_some() {
            hits += 1;
        }
    }
    let lookup = started.elapsed();
    assert_eq!(hits, tracks);

    // An unchanged rescan: touches only.
    let started = Instant::now();
    let ids: Vec<u64> = store.inspect_tracks(&lib.id).iter().map(|t| t.id).collect();
    for chunk in ids.chunks(batch_size) {
        let mut batch = Batch::new(scan_id + 1);
        batch.touched.extend_from_slice(chunk);
        store.apply(&lib.id, batch).unwrap();
    }
    let touch = started.elapsed();

    let per_500k = |d: std::time::Duration| d.as_secs_f64() * 500_000.0 / tracks as f64;
    eprintln!(
        "{tracks} tracks: insert {insert:.2?} ({:.0}s per 500k), lookup {lookup:.2?} ({:.0}s per 500k), touch-only rescan {touch:.2?} ({:.0}s per 500k)",
        per_500k(insert),
        per_500k(lookup),
        per_500k(touch)
    );
}
