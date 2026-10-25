//! The hourly sweep: each kind of row goes once it is older than the window and stays until
//! then, each library on its own, and a large sweep runs a chunk at a time.

use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Connection, Result, params};

use super::catalog::{self, MISSING, TRACK_CHUNK};
use super::{CHUNK, WINDOW_MS, scans, start};
use crate::db::Database;
use crate::db::testing::{self, Album, Artist, Image, Track, User, library, sweep};

/// A day, in milliseconds.
const DAY: i64 = 24 * 60 * 60 * 1000;
/// When the sweep runs.
const NOW: i64 = 1000 * DAY;
/// Just past the window.
const OLD: i64 = NOW - WINDOW_MS - 1;
/// Well inside the window.
const RECENT: i64 = NOW - 29 * DAY;

/// A database with libraries 1 and 2 and user 1, after `setup`.
fn open(
    setup: impl FnOnce(&Connection) -> Result<()> + Send + 'static,
) -> (tempfile::TempDir, Arc<Database>) {
    let (temp, db) = testing::open();
    db.write_blocking(|tx| {
        library(tx, 1, "One", "/one")?;
        library(tx, 2, "Two", "/two")?;
        User::new(1, "owner").insert(tx)?;
        setup(tx)
    })
    .unwrap();
    (temp, db)
}

/// The first column of every row `sql` returns.
fn column(db: &Database, sql: &'static str) -> Vec<i64> {
    db.read_blocking(move |conn| conn.prepare(sql)?.query_map([], |r| r.get(0))?.collect()).unwrap()
}

/// Adds scan `id` of `library` in `state`, made at `created` and finished then too, unless it
/// is running.
fn scan(conn: &Connection, id: i64, library: i64, state: &str, created: i64) -> Result<()> {
    let finished = (state != "running").then_some(created);
    conn.execute(
        "INSERT INTO scans (id, library_id, trigger, state, scopes, created_at, finished_at) \
         VALUES (?1, ?2, 'manual', ?3, '[]', ?4, ?5)",
        params![id, library, state, created, finished],
    )
    .map(drop)
}

/// Finished scans past the window go, but each library keeps its last finished scan and its
/// last completed one, however old, whatever the other library holds. A running scan stays.
#[test]
fn finished_scans_age_out_but_each_library_keeps_its_last() {
    let (_temp, db) = open(|conn| {
        scan(conn, 1, 1, "completed", NOW - 45 * DAY)?;
        scan(conn, 2, 1, "completed", OLD)?;
        scan(conn, 3, 1, "suspended", NOW - 35 * DAY)?;
        scan(conn, 4, 1, "cancelled", RECENT)?;
        scan(conn, 5, 1, "running", NOW - 40 * DAY)?;
        scan(conn, 9, 1, "completed", NOW - 28 * DAY)?;
        scan(conn, 6, 2, "completed", NOW - 50 * DAY)?;
        scan(conn, 7, 2, "cancelled", NOW - 45 * DAY)?;
        scan(conn, 8, 2, "cancelled", NOW - 40 * DAY)
    });
    sweep(&db, NOW);
    assert_eq!(column(&db, "SELECT id FROM scans ORDER BY id"), [4, 5, 6, 8, 9]);
}

#[test]
fn login_attempts_age_out() {
    let (_temp, db) = open(|conn| {
        for (id, at) in [(1, OLD), (2, RECENT)] {
            conn.execute(
                "INSERT INTO login_attempts (id, attempted_at, username_attempted, origin, \
                 succeeded) VALUES (?1, ?2, 'user1', '192.0.2.1', 0)",
                [id, at],
            )?;
        }
        Ok(())
    });
    sweep(&db, NOW);
    assert_eq!(column(&db, "SELECT id FROM login_attempts"), [2]);
}

/// The `played` hook's ends age out, and the plays themselves stay.
#[test]
fn play_ends_age_out_and_plays_stay() {
    let (_temp, db) = open(|conn| {
        for (id, at) in [("1", OLD), ("2", RECENT)] {
            conn.execute(
                "INSERT INTO plays (id, user_id, library_id, track_id, title, artists, artist_keys, \
                 album_title_key, album_artists_key, genre_keys, duration_us, started_at, \
                 listen_time_ms, ended, context, origin, updated_at) \
                 VALUES (?1, 1, 1, 7, 'T', '[]', '[]', '', '', '[]', 1000, ?2, 0, 'finished', \
                         '{}', 'manual', ?2)",
                params![id, at],
            )?;
        }
        Ok(())
    });
    sweep(&db, NOW);
    assert_eq!(column(&db, "SELECT CAST(play_id AS INTEGER) FROM play_events"), [2]);
    assert_eq!(column(&db, "SELECT count(*) FROM plays"), [2]);
}

/// Adds a `kind` row, `searched` or `forgotten`, to the `searched` hook's outbox for user 1's
/// search 1 in `library` at `at`.
fn noted(conn: &Connection, library: i64, kind: &str, at: i64) -> Result<()> {
    let found = (kind == "searched").then_some(0);
    conn.execute(
        "INSERT INTO search_events (user_id, library_id, kind, search_id, found_tracks, \
         found_albums, found_artists, created_at) VALUES (1, ?1, ?2, 1, ?3, ?3, ?3, ?4)",
        params![library, kind, found, at],
    )
    .map(drop)
}

/// Adds `user`'s request to `plugin` to forget their searches in `library` at `at`, and
/// answers its position in the outbox.
fn stopped(conn: &Connection, library: i64, user: i64, plugin: &str, at: i64) -> Result<i64> {
    conn.query_row(
        "INSERT INTO search_events (user_id, library_id, kind, plugin_id, created_at) \
         VALUES (?1, ?2, 'stopped', ?3, ?4) RETURNING seq",
        params![user, library, plugin, at],
        |r| r.get(0),
    )
}

/// Sets plugin `p`'s position in `hook` for user 1 in `library`.
fn delivered(conn: &Connection, library: i64, hook: &str, position: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO plugin_cursors (plugin_id, library_id, hook, user_id, position) \
         VALUES ('p', ?1, ?2, 1, ?3)",
        params![library, hook, position],
    )
    .map(drop)
}

/// Recent searches and what the `searched` hook noted of them age out together. A request to
/// forget is kept until its own plugin's `searched` position for its own user in its own
/// library reaches it, however old it is, and goes once it has, however recent.
#[test]
fn searches_age_out_and_a_request_to_forget_waits_until_it_is_delivered() {
    let (_temp, db) = open(|conn| {
        conn.execute("INSERT INTO plugins (id, manifest, installed_at) VALUES ('p', '{}', 0)", [])?;
        User::new(2, "user").insert(conn)?;
        for (id, at) in [(1, OLD), (2, RECENT)] {
            conn.execute(
                "INSERT INTO recent_searches (id, user_id, library_id, query, folded, searched_at) \
                 VALUES (?1, 1, 1, 'q' || ?1, 'q' || ?1, ?2)",
                [id, at],
            )?;
        }
        noted(conn, 1, "searched", OLD)?;
        noted(conn, 1, "forgotten", RECENT)?;
        let waiting = stopped(conn, 1, 1, "p", NOW - 60 * DAY)?;
        delivered(conn, 1, "searched", waiting - 1)?;
        // Another hook's position delivers nothing of it.
        delivered(conn, 1, "played", i64::MAX)?;
        // Plugin p's position for user 1 delivers nothing of another plugin's or user's.
        stopped(conn, 2, 1, "q", NOW - DAY)?;
        stopped(conn, 2, 2, "p", NOW - DAY)?;
        let sent = stopped(conn, 2, 1, "p", NOW - DAY)?;
        delivered(conn, 2, "searched", sent)
    });
    sweep(&db, NOW);
    assert_eq!(column(&db, "SELECT id FROM recent_searches"), [2]);
    let kept = db
        .read_blocking(|conn| {
            conn.prepare(
                "SELECT kind, library_id, user_id, plugin_id FROM search_events ORDER BY seq",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<Vec<(String, i64, i64, Option<String>)>>>()
        })
        .unwrap();
    let row = |kind: &str, library, user, plugin: Option<&str>| {
        (kind.to_owned(), library, user, plugin.map(str::to_owned))
    };
    assert_eq!(
        kept,
        [
            row("forgotten", 1, 1, None),
            row("stopped", 1, 1, Some("p")),
            row("stopped", 2, 1, Some("q")),
            row("stopped", 2, 2, Some("p")),
        ]
    );
}

/// Adds image `id` of `library` and biography `id`, neither used yet.
fn art(conn: &Connection, library: i64, id: i64) -> Result<()> {
    Image::new(id, library, "a.png").insert(conn)?;
    conn.execute(
        "INSERT INTO biographies (id, library_id, hash, text) VALUES (?1, ?2, ?3, 'Bio')",
        params![id, library, id.to_be_bytes()],
    )
    .map(drop)
}

/// An image goes once no track, album or artist uses it, and a biography once no track does,
/// in each library.
#[test]
fn unused_art_and_biographies_go() {
    let (_temp, db) = open(|conn| {
        for id in 1..=5 {
            art(conn, 1, id)?;
        }
        art(conn, 2, 6)?;
        art(conn, 2, 7)?;
        Album::new(10, 1).insert(conn)?;
        Track::new(20, 1, 10).insert(conn)?;
        Artist::new(30, 1, "A").insert(conn)?;
        Album::new(11, 2).insert(conn)?;
        Track::new(21, 2, 11).insert(conn)?;
        conn.execute_batch(
            "UPDATE tracks SET image_id = 1, artist_image_id = 2, biography_id = 1 WHERE id = 20; \
             UPDATE albums SET image_id = 3 WHERE id = 10; \
             UPDATE artists SET image_id = 4 WHERE id = 30; \
             UPDATE tracks SET image_id = 7, biography_id = 7 WHERE id = 21;",
        )
    });
    sweep(&db, NOW);
    assert_eq!(column(&db, "SELECT id FROM images ORDER BY id"), [1, 2, 3, 4, 7]);
    assert_eq!(column(&db, "SELECT id FROM biographies ORDER BY id"), [1, 7]);
}

/// Marks every root as walked whole by a scan that completed after starting at `at`.
fn walked(conn: &Connection, at: i64) -> Result<()> {
    conn.execute("UPDATE library_roots SET walked_at = ?1", [at]).map(drop)
}

/// Tracks missing past the window are purged, each recorded as deleted in its own library's
/// feed; those missing for less, for exactly the window, and those present, stay.
#[test]
fn missing_tracks_go_after_the_window_in_each_library() {
    let (_temp, db) = open(|conn| {
        walked(conn, NOW)?;
        Album::new(10, 1).insert(conn)?;
        Album::new(11, 2).insert(conn)?;
        for (id, library, album, missing) in [
            (1, 1, 10, Some(OLD)),
            (2, 1, 10, Some(RECENT)),
            (3, 1, 10, None),
            (4, 2, 11, Some(RECENT)),
            (5, 2, 11, Some(OLD)),
            (6, 1, 10, Some(NOW - WINDOW_MS)),
        ] {
            Track { missing_since: missing, ..Track::new(id, library, album) }.insert(conn)?;
        }
        Ok(())
    });
    sweep(&db, NOW);
    assert_eq!(column(&db, "SELECT id FROM tracks ORDER BY id"), [2, 3, 4, 6]);
    let deleted = db
        .read_blocking(|conn| {
            conn.prepare(
                "SELECT library_id, entity_id FROM library_changes \
                 WHERE entity_type = 'track' AND op = 'delete' ORDER BY entity_id",
            )?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>>>()
        })
        .unwrap();
    assert_eq!(deleted, [(1, 1), (2, 5)]);
}

/// A track missing past the window stays until a scan of its whole root that started once the
/// window was up has completed: not while no scan has, nor after one that started a moment
/// before (`requirements/scanning.md` §8). A track missing for less stays, though a scan's start
/// lies a window after its mark, as when the clock was ahead as the scan started.
#[test]
fn a_missing_track_waits_for_a_scan_after_the_window() {
    let (_temp, db) = open(|conn| {
        Album::new(10, 1).insert(conn)?;
        Track { missing_since: Some(OLD - DAY), ..Track::new(1, 1, 10) }.insert(conn)?;
        Track { missing_since: Some(RECENT), ..Track::new(2, 1, 10) }.insert(conn)
    });
    let tracks = "SELECT id FROM tracks ORDER BY id";
    sweep(&db, NOW);
    assert_eq!(column(&db, tracks), [1, 2], "no scan has completed");
    db.write_blocking(|tx| walked(tx, OLD - DAY + WINDOW_MS)).unwrap();
    sweep(&db, NOW);
    assert_eq!(column(&db, tracks), [1, 2], "the scan started as the window ended");
    db.write_blocking(|tx| walked(tx, RECENT + WINDOW_MS + 1)).unwrap();
    sweep(&db, NOW);
    assert_eq!(column(&db, tracks), [2]);
}

/// The purge reads each root's marks through `tracks_missing`, so marks under a root not yet
/// walked again cost nothing to the roots that were.
#[test]
fn the_purge_reads_the_marks_root_by_root() {
    let (_temp, db) = open(|_| Ok(()));
    let plan: Vec<String> = db
        .read_blocking(|conn| {
            conn.prepare(&format!("EXPLAIN QUERY PLAN {MISSING}"))?
                .query_map(params![1, 0, 1, 0], |r| r.get(3))?
                .collect()
        })
        .unwrap();
    let by_root = "COVERING INDEX tracks_missing (root_id=? AND missing_since<?)";
    assert!(plan.iter().any(|step| step.contains(by_root)), "{plan:?}");
}

/// A sweep with more to delete than a chunk deletes it a chunk at a time: one write of a stage
/// purges at most [`TRACK_CHUNK`] tracks, or looks at no more than [`CHUNK`] images, and the
/// sweep ends with what is used still there: the present tracks, and the image each of them
/// shows.
#[test]
fn a_large_sweep_goes_a_chunk_at_a_time() {
    let n = i64::try_from(CHUNK).unwrap() * 2 + 50;
    let (_temp, db) = open(move |conn| {
        walked(conn, NOW)?;
        Album::new(10, 1).insert(conn)?;
        for id in 1..=n {
            let missing = (id % 2 == 1).then_some(OLD);
            Track { missing_since: missing, ..Track::new(id, 1, 10) }.insert(conn)?;
            Image::new(id, 1, "a.png").insert(conn)?;
        }
        conn.execute("UPDATE tracks SET image_id = id WHERE missing_since IS NULL", []).map(drop)
    });
    let tracks = "SELECT count(*) FROM tracks";
    let before = column(&db, tracks)[0];
    let done = db.write_blocking(|tx| catalog::tracks(tx, NOW, &mut 0)).unwrap();
    assert!(!done);
    assert_eq!(before - column(&db, tracks)[0], i64::try_from(TRACK_CHUNK).unwrap());
    let images = "SELECT count(*) FROM images";
    let before = column(&db, images)[0];
    let done = db.write_blocking(|tx| catalog::images(tx, NOW, &mut 0)).unwrap();
    assert!(!done);
    assert_eq!(before - column(&db, images)[0], i64::try_from(CHUNK).unwrap() / 2);

    sweep(&db, NOW);
    let even = (2..=n).step_by(2).collect::<Vec<_>>();
    assert_eq!(column(&db, "SELECT id FROM tracks ORDER BY id"), even);
    assert_eq!(column(&db, "SELECT id FROM images ORDER BY id"), even);
}

/// Writes queued while a large purge is under way run between its chunks, not after the whole
/// purge: the writes see its progress.
#[tokio::test]
async fn a_long_purge_leaves_the_writer_free_between_chunks() {
    let n = 128;
    // The helpers that set a database up block, so they run outside the runtime.
    let (_temp, db) = tokio::task::spawn_blocking(move || {
        open(move |conn| {
            walked(conn, NOW)?;
            Album::new(10, 1).insert(conn)?;
            for id in 1..=n {
                Track { missing_since: Some(OLD), ..Track::new(id, 1, 10) }.insert(conn)?;
            }
            Ok(())
        })
    })
    .await
    .unwrap();
    let mut purging = tokio::spawn({
        let db = Arc::clone(&db);
        async move { super::sweep(&db, NOW).await }
    });
    // Writes queued as it runs each see the tracks it has purged so far.
    let mut seen = std::collections::BTreeSet::new();
    while !purging.is_finished() {
        let left: i64 = db
            .write(|tx| tx.query_row("SELECT count(*) FROM tracks", [], |r| r.get(0)))
            .await
            .unwrap();
        seen.insert(left);
    }
    (&mut purging).await.unwrap();
    assert!(seen.len() > 2, "the writer was not free between chunks: saw {seen:?}");
    assert_eq!(
        db.read(|c| c.query_row("SELECT count(*) FROM tracks", [], |r| r.get::<_, i64>(0)))
            .await
            .unwrap(),
        0
    );
}

/// A kind of row that cannot be swept is passed over, and the kinds after it are swept.
#[test]
fn a_kind_that_fails_is_passed_over() {
    let (_temp, db) = open(|conn| {
        conn.execute_batch(
            "INSERT INTO login_attempts (attempted_at, username_attempted, origin, succeeded) \
             VALUES (0, 'user1', '192.0.2.1', 0); \
             INSERT INTO recent_searches (user_id, library_id, query, folded, searched_at) \
             VALUES (1, 1, 'q', 'q', 0); \
             CREATE TRIGGER poison BEFORE DELETE ON login_attempts \
             BEGIN SELECT RAISE(ABORT, 'poison'); END;",
        )
    });
    sweep(&db, NOW);
    assert_eq!(column(&db, "SELECT count(*) FROM login_attempts"), [1]);
    assert_eq!(column(&db, "SELECT count(*) FROM recent_searches"), [0]);
}

/// One write deletes no more than [`CHUNK`] old scans in all, however many libraries hold them.
#[test]
fn a_chunk_deletes_no_more_scans_than_the_chunk_across_libraries() {
    let n = i64::try_from(CHUNK).unwrap() * 6 / 10;
    let (_temp, db) = open(move |conn| {
        for library in [1, 2] {
            for i in 0..=n {
                scan(conn, library * 1000 + i, library, "completed", OLD - i)?;
            }
        }
        Ok(())
    });
    let count = "SELECT count(*) FROM scans";
    db.write_blocking(|tx| scans(tx, NOW, &mut 0).map(drop)).unwrap();
    assert_eq!(column(&db, count), [2 * n + 2 - i64::try_from(CHUNK).unwrap()]);
    sweep(&db, NOW);
    assert_eq!(column(&db, count), [2], "each library's last");
}

/// The task sweeps on its own once its first sweep's time comes, and stops when aborted.
#[tokio::test]
async fn the_task_sweeps_on_its_own() {
    let (_temp, db) = tokio::task::spawn_blocking(|| {
        open(|conn| {
            conn.execute(
                "INSERT INTO login_attempts (attempted_at, username_attempted, origin, \
                 succeeded) VALUES (0, 'user1', '192.0.2.1', 0)",
                [],
            )
            .map(drop)
        })
    })
    .await
    .unwrap();
    let task = start(Arc::clone(&db), Duration::ZERO);
    let swept = async {
        loop {
            let left: i64 = db
                .read(|c| c.query_row("SELECT count(*) FROM login_attempts", [], |r| r.get(0)))
                .await
                .unwrap();
            if left == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), swept).await.expect("not swept after 10 s");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}
