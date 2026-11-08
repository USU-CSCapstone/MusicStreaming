//! The device rows of a scanned library (`design/general.md` §3): the device schema is a subset
//! of the server's, an entity's rows survive the trip to a device, and only device columns are
//! exported (`requirements/general.md` §3.7).

use std::collections::{BTreeMap, BTreeSet};

use jewelcase_catalog::sync::{Kind, Rows, apply, delete, export};
use jewelcase_catalog::{SCHEMA, SCHEMA_VERSION, create_schema};
use jewelcase_scanner::testing::{self, ItemKey, PNG};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use super::{make, tagged};
use crate::db::store::SqliteStore;

/// The columns the server's tables have and the device's do not, per table: paths and roots,
/// file stamps, scan and analysis state, plugin data, and keys the server derives. A column
/// joins the device schema only by leaving this list, and a new server column must be put on one
/// side or the other.
const SERVER_ONLY: &[(&str, &[&str])] = &[
    ("albums", &["artists_key", "title_key"]),
    ("artists", &["name_key"]),
    ("images", &["hash", "path", "root_id"]),
    ("tag_types", &["field", "plugin_id", "since"]),
    ("tags", &["name_key"]),
    (
        "tracks",
        &[
            "album_artist_ids",
            "album_title",
            "analyzer_version",
            "artist_image_id",
            "biography_id",
            "compilation",
            "file_album_artist_sort_key",
            "file_album_sort_key",
            "file_artist_sort_key",
            "file_mtime",
            "identifiers",
            "image_embedded",
            "isrc",
            "labels",
            "path",
            "release_type",
            "root_id",
            "sidecar_stamp",
            "tags_generation",
        ],
    ),
];

/// A scanned library of two albums, one with art, with a second artist on a track, genres, and a
/// placeholder on the image.
fn fixture() -> (tempfile::TempDir, SqliteStore) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    let one = [
        (ItemKey::TrackTitle, "One"),
        (ItemKey::TrackArtist, "Alpha; Beta"),
        (ItemKey::AlbumTitle, "Red"),
        (ItemKey::Genre, "Rock"),
    ];
    let path = root.join("red/01.wav");
    testing::write_small_wav(&path);
    testing::tag_wav_with_art(&path, &one, PNG);
    tagged(
        &root,
        "red/02.wav",
        &[
            (ItemKey::TrackTitle, "Two"),
            (ItemKey::TrackArtist, "Alpha"),
            (ItemKey::AlbumTitle, "Red"),
        ],
    );
    tagged(
        &root,
        "blue/01.wav",
        &[
            (ItemKey::TrackTitle, "Three"),
            (ItemKey::TrackArtist, "Gamma"),
            (ItemKey::AlbumTitle, "Blue"),
            (ItemKey::Genre, "Jazz"),
        ],
    );
    let store = make(&root);
    testing::scan_library(&store, &testing::library(&root));
    store
        .db()
        .write_blocking(|tx| tx.execute("UPDATE images SET placeholder = x'00ff10'", []).map(drop))
        .unwrap();
    (tmp, store)
}

/// A device database with the schema.
fn device() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    create_schema(&conn).unwrap();
    conn
}

/// The `(name, type)` of each column of each table `conn` has.
fn schema(
    conn: &Connection,
    only: Option<&BTreeSet<String>>,
) -> BTreeMap<String, Vec<(String, String)>> {
    let mut out = BTreeMap::<String, Vec<(String, String)>>::new();
    let mut stmt = conn
        .prepare(
            "SELECT m.name, p.name, p.type FROM sqlite_schema AS m, pragma_table_info(m.name) AS p \
             WHERE m.type = 'table' ORDER BY m.name, p.cid",
        )
        .unwrap();
    for row in stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap() {
        let (table, name, ty): (String, String, String) = row.unwrap();
        if only.is_none_or(|tables| tables.contains(&table)) {
            out.entry(table).or_default().push((name, ty));
        }
    }
    out
}

/// Every entity of library 1, from the server's tables.
fn entities(conn: &Connection) -> Vec<(Kind, String)> {
    let mut all = Vec::new();
    for (kind, sql) in [
        (Kind::Artist, "SELECT id FROM artists"),
        (Kind::Album, "SELECT id FROM albums"),
        (Kind::Track, "SELECT id FROM tracks"),
        (Kind::Tag, "SELECT id FROM tags"),
    ] {
        let mut stmt = conn.prepare(sql).unwrap();
        let ids = stmt.query_map([], |r| r.get::<_, i64>(0)).unwrap();
        for id in ids {
            all.push((kind, id.unwrap().to_string()));
        }
    }
    all
}

#[test]
fn every_device_column_exists_in_the_server_schema_with_the_same_type() {
    let (_tmp, store) = fixture();
    let device = schema(&device(), None);
    let tables: BTreeSet<String> = device.keys().cloned().collect();
    let server = store.db().read_blocking(move |conn| Ok(schema(conn, Some(&tables)))).unwrap();
    assert_eq!(server.keys().collect::<Vec<_>>(), device.keys().collect::<Vec<_>>());
    for (table, columns) in &device {
        for column in columns {
            assert!(server[table].contains(column), "{table}.{} is not on the server", column.0);
        }
    }
    assert_eq!(device.len(), 13);
}

#[test]
fn the_device_schema_changes_only_with_its_version() {
    let hash = format!("{:x}", Sha256::digest(SCHEMA));
    assert_eq!(
        (SCHEMA_VERSION, hash.as_str()),
        (1, "24aa344b0ec8b9f8a8595f2b155471eac50648e95fbb8d7bce460e03092ea1eb"),
        "schema.sql changed: raise SCHEMA_VERSION, and pin its new hash here"
    );
}

#[test]
fn no_server_only_column_is_exported() {
    let (_tmp, store) = fixture();
    let device = schema(&device(), None);
    let widths: BTreeMap<_, _> = device.iter().map(|(t, c)| (t.clone(), c.len())).collect();
    let tables: BTreeSet<String> = device.keys().cloned().collect();
    let server = store.db().read_blocking(move |conn| Ok(schema(conn, Some(&tables)))).unwrap();
    let left_out: BTreeMap<&str, Vec<&str>> = server
        .iter()
        .map(|(table, columns)| {
            let only = columns.iter().filter(|c| !device[table].contains(c));
            let mut only: Vec<&str> = only.map(|(name, _)| name.as_str()).collect();
            only.sort_unstable();
            (table.as_str(), only)
        })
        .filter(|(_, only)| !only.is_empty())
        .collect();
    let pinned: BTreeMap<&str, Vec<&str>> =
        SERVER_ONLY.iter().map(|(table, columns)| (*table, columns.to_vec())).collect();
    assert_eq!(left_out, pinned, "the device schema leaves out exactly the server-only columns");
    // Every row is exactly as wide as the device table, so no column of the server's is in it,
    // and the entities between them cover the whole device schema.
    let exported = store
        .db()
        .read_blocking(|conn| {
            Ok(entities(conn)
                .iter()
                .map(|(kind, id)| export(conn, 1, *kind, id).unwrap().unwrap())
                .collect::<Vec<Rows>>())
        })
        .unwrap();
    let mut seen = BTreeSet::new();
    for rows in &exported {
        for (table, list) in &rows.tables {
            seen.insert(table.clone());
            assert!(list.iter().all(|row| row.len() == widths[table]), "{table}");
        }
    }
    assert_eq!(seen.len(), 13, "{seen:?}");
}

#[test]
fn export_then_apply_reproduces_each_entitys_device_rows() {
    let (_tmp, store) = fixture();
    let device = device();
    store
        .db()
        .read_blocking(move |conn| {
            let all = entities(conn);
            assert_eq!(all.len(), 10, "{all:?}");
            for (kind, id) in &all {
                let rows = export(conn, 1, *kind, id).unwrap().unwrap();
                // Through JSON, as the change feed carries it.
                let wire = serde_json::to_string(&rows).unwrap();
                let rows: Rows = serde_json::from_str(&wire).unwrap();
                apply(&device, &rows).unwrap();
                assert_eq!(export(&device, 1, *kind, id).unwrap().unwrap(), rows, "{kind:?} {id}");
            }
            // An entity applied again is replaced, not doubled.
            let (kind, id) = all.iter().find(|(kind, _)| *kind == Kind::Track).unwrap();
            let rows = export(conn, 1, *kind, id).unwrap().unwrap();
            apply(&device, &rows).unwrap();
            assert_eq!(export(&device, 1, *kind, id).unwrap().unwrap(), rows);
            Ok(())
        })
        .unwrap();
}

#[test]
fn ids_travel_as_strings_and_blobs_as_base64() {
    let (_tmp, store) = fixture();
    let (id, wire) = store
        .db()
        .read_blocking(|conn| {
            let id: i64 =
                conn.query_row("SELECT id FROM tracks WHERE title = 'One'", [], |r| r.get(0))?;
            let rows = export(conn, 1, Kind::Track, &id.to_string()).unwrap().unwrap();
            Ok((id, serde_json::to_value(rows).unwrap()))
        })
        .unwrap();
    assert_eq!(wire["kind"], "track");
    assert_eq!(wire["id"], id.to_string());
    let track = &wire["tables"]["tracks"][0];
    assert_eq!(track["id"], id.to_string());
    assert_eq!(track["library_id"], "1");
    assert!(track["duration_us"].is_number(), "other integers stay numbers");
    let image = &wire["tables"]["images"][0];
    assert_eq!(image["id"], track["image_id"], "the track's image_id");
    assert_eq!(image["placeholder"], "AP8Q", "the placeholder");
}

#[test]
fn applying_replaces_an_entitys_join_rows_and_deleting_removes_them() {
    let (_tmp, store) = fixture();
    let device = device();
    let (rows, id) = store
        .db()
        .read_blocking(|conn| {
            let id: i64 =
                conn.query_row("SELECT id FROM tracks WHERE title = 'One'", [], |r| r.get(0))?;
            Ok((export(conn, 1, Kind::Track, &id.to_string()).unwrap().unwrap(), id.to_string()))
        })
        .unwrap();
    assert_eq!(rows.tables["track_artists"].len(), 2);
    apply(&device, &rows).unwrap();
    device
        .execute(
            "INSERT INTO track_artists VALUES (1, ?1, 7, 9, 'Stale')",
            [id.parse::<i64>().unwrap()],
        )
        .unwrap();
    apply(&device, &rows).unwrap();
    assert_eq!(
        export(&device, 1, Kind::Track, &id).unwrap().unwrap(),
        rows,
        "the stale credit is gone"
    );

    delete(&device, Kind::Track, id.parse().unwrap()).unwrap();
    assert_eq!(export(&device, 1, Kind::Track, &id).unwrap(), None);
    for table in ["tracks", "track_artists", "track_tags"] {
        let left: i64 =
            device.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "{table}");
    }
    let images: i64 = device.query_row("SELECT count(*) FROM images", [], |r| r.get(0)).unwrap();
    assert_eq!(images, 1, "the image stays, as other entities may show it");
}

#[test]
fn another_librarys_entity_is_absent() {
    let (_tmp, store) = fixture();
    store
        .db()
        .read_blocking(|conn| {
            let id: i64 = conn.query_row("SELECT id FROM tracks LIMIT 1", [], |r| r.get(0))?;
            assert!(export(conn, 2, Kind::Track, &id.to_string()).unwrap().is_none());
            assert!(export(conn, 1, Kind::Track, "not an id").unwrap().is_none());
            Ok(())
        })
        .unwrap();
}
