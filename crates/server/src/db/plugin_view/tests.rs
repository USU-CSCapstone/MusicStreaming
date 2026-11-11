//! The fingerprint covers what plugins read and nothing devices alone are given.

use std::collections::BTreeSet;

use rusqlite::{Connection, Transaction};

use super::{ALBUM, ARTIST, TRACK, fingerprint};
use crate::db::feed::Entity;
use crate::db::testing::{Album, Artist, Image, Track, album_artist, library, open, track_artist};

/// Track 11 on album 101 by artist 301 in library 1, all three showing image 1, and the same
/// again in library 2 so a change to every row in a table reaches across libraries too.
fn catalog(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    for lib in [1, 2] {
        library(tx, lib, &format!("Music {lib}"), format!("/music{lib}"))?;
        let (image, album, artist, track) = (lib, 100 * lib + 1, 300 * lib + 1, 10 * lib + 1);
        Image::new(image, lib, "cover.png").insert(tx)?;
        Album { title: Some("Signal".into()), track_count: 1, ..Album::new(album, lib) }
            .insert(tx)?;
        Artist::new(artist, lib, "Aurora Lane").insert(tx)?;
        album_artist(tx, lib, album, artist)?;
        Track::new(track, lib, album).insert(tx)?;
        track_artist(tx, lib, track, 0, artist, "Aurora Lane")?;
        for table in ["tracks", "albums", "artists"] {
            tx.execute(&format!("UPDATE {table} SET image_id = ?1 WHERE library_id = ?1"), [lib])?;
        }
    }
    Ok(())
}

/// What plugins see of library 1's track, album and artist.
fn fingerprints(conn: &Connection) -> rusqlite::Result<[Option<i64>; 3]> {
    Ok([
        fingerprint(conn, 1, Entity::Track, 11)?,
        fingerprint(conn, 1, Entity::Album, 101)?,
        fingerprint(conn, 1, Entity::Artist, 301)?,
    ])
}

/// Every word in the views' SQL, which is every column a view could read, and more.
fn read_by_views() -> BTreeSet<String> {
    [&TRACK, &ALBUM, &ARTIST]
        .iter()
        .flat_map(|view| view.observed().into_iter().chain([view.rest]))
        .flat_map(|sql| sql.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')))
        .map(str::to_owned)
        .collect()
}

/// Every column of the device's copy of `tracks`, `albums`, `artists` and `images` that no view
/// names can change without moving any fingerprint, so a column added for devices can never wake
/// a plugin. The list is read from the device schema, so a new column is covered as it is added.
#[test]
fn no_column_given_only_to_devices_moves_a_fingerprint() {
    let device = Connection::open_in_memory().unwrap();
    jewelcase_catalog::create_schema(&device).unwrap();
    let named = read_by_views();
    let mut walked = Vec::new();
    for table in ["tracks", "albums", "artists", "images"] {
        let columns: Vec<(String, String)> = device
            .prepare("SELECT name, type FROM pragma_table_info(?1)")
            .unwrap()
            .query_map([table], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for (column, kind) in columns {
            if column != "library_id" && !named.contains(&column) {
                walked.push((table, column, kind));
            }
        }
    }
    let names: Vec<&str> = walked.iter().map(|(_, column, _)| column.as_str()).collect();
    for expected in
        ["placeholder", "artist_sort_key", "album_sort_key", "loudness_lufs", "added_at"]
    {
        assert!(names.contains(&expected), "{expected} is walked: {names:?}");
    }

    let (_temp, db) = open();
    db.write_blocking(move |tx| {
        catalog(tx)?;
        let before = fingerprints(tx)?;
        assert!(before.iter().all(|f| f.is_some_and(|f| f != 0)), "all three seen: {before:?}");
        // Any value of the column's type will do, so its checks and keys are set aside, and each
        // change is undone before the next.
        tx.pragma_update(None, "ignore_check_constraints", true)?;
        tx.pragma_update(None, "defer_foreign_keys", true)?;
        for (table, column, kind) in &walked {
            let changed = match kind.as_str() {
                "INTEGER" => format!("coalesce({column}, 0) + 1"),
                "REAL" => format!("coalesce({column}, 0) + 1.5"),
                "TEXT" => format!("coalesce({column}, '') || 'x'"),
                "BLOB" => format!("CAST(coalesce({column}, x'') || 'x' AS BLOB)"),
                other => panic!("{table}.{column} has type {other}"),
            };
            tx.execute_batch("SAVEPOINT walk")?;
            tx.execute(&format!("UPDATE {table} SET {column} = {changed}"), [])?;
            assert_eq!(fingerprints(tx)?, before, "{table}.{column} moved a fingerprint");
            tx.execute_batch("ROLLBACK TO walk; RELEASE walk")?;
        }
        tx.pragma_update(None, "ignore_check_constraints", false)
    })
    .unwrap();
}

/// What plugins read moves the fingerprint: a title, an artist's name, an album's and an artist's
/// counts, a file stamp, and leaving the view.
#[test]
fn what_plugins_read_moves_its_fingerprint() {
    let (_temp, db) = open();
    db.write_blocking(|tx| {
        catalog(tx)?;
        let changes = [
            (0, "UPDATE tracks SET title = 'Other' WHERE id = 11"),
            (0, "UPDATE track_artists SET artist_name = 'Other' WHERE track_id = 11"),
            (0, "UPDATE albums SET title = 'Other' WHERE id = 101"),
            (0, "UPDATE tracks SET sidecar_stamp = 1 WHERE id = 11"),
            (0, "UPDATE tracks SET missing_since = 1 WHERE id = 11"),
            (1, "UPDATE artists SET name = 'Other' WHERE id = 301"),
            (1, "UPDATE albums SET available_track_count = available_track_count + 1 WHERE id = 101"),
            (1, "UPDATE albums SET image_id = NULL WHERE id = 101"),
            (1, "UPDATE albums SET release_date = '1999' WHERE id = 101"),
            (2, "UPDATE artists SET album_count = album_count + 1 WHERE id = 301"),
            (2, "UPDATE artists SET biography = 'Born.' WHERE id = 301"),
        ];
        tx.pragma_update(None, "defer_foreign_keys", true)?;
        for (index, sql) in changes {
            let before = fingerprints(tx)?;
            tx.execute_batch("SAVEPOINT change")?;
            tx.execute(sql, [])?;
            assert_ne!(fingerprints(tx)?[index], before[index], "{sql}");
            tx.execute_batch("ROLLBACK TO change; RELEASE change")?;
        }
        Ok(())
    })
    .unwrap();
}
