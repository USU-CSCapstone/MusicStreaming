//! The catalog's shared queries give the same JSON on the server's database and on a device
//! database synced from it (`design/general.md` §3). The answers are also written to
//! `crates/catalog/parity/`, with the rows that built the device, for `catalog-wasm` to replay,
//! and the library as a snapshot file for the web client's sync tests (`UPDATE_PARITY=1` rewrites
//! them).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;

use jewelcase_catalog::sync::{Kind, Rows, apply, export, export_for};
use jewelcase_catalog::{create_schema, mutate, query, search};
use rusqlite::Connection;
use serde_json::{Value, json};

use super::super::testing::{TOKEN, call, catalog};
use crate::db::Database;
use crate::db::testing::User;

/// The library every query asks, and the account whose playlists it asks for.
const LIBRARY: i64 = 1;
const USER: i64 = 1;

/// Answers the query `name` with `params` from `conn`, as the API would send it, or `{error}`
/// where it is refused. The server and a device each build their search index from their own
/// documents.
fn answer(conn: &Connection, name: &str, params: &Value) -> Value {
    let index = search::index(conn, LIBRARY).unwrap();
    match query::answer(conn, USER, LIBRARY, name, &params.to_string(), Some(&index), None) {
        Ok(text) => serde_json::from_str(&text).expect("an answer is JSON"),
        Err(error) => json!({ "error": error.to_string() }),
    }
}

/// Every device row of `LIBRARY`, as the library feed carries each entity.
fn feed(conn: &Connection) -> Vec<Rows> {
    let mut rows = Vec::new();
    for (kind, sql) in [
        (Kind::Library, "SELECT id FROM libraries WHERE id = 1"),
        (Kind::Artist, "SELECT id FROM artists WHERE library_id = 1 ORDER BY id"),
        (Kind::Album, "SELECT id FROM albums WHERE library_id = 1 ORDER BY id"),
        (Kind::Track, "SELECT id FROM tracks WHERE library_id = 1 ORDER BY id"),
        (Kind::Tag, "SELECT id FROM tags WHERE library_id = 1 ORDER BY id"),
    ] {
        let ids: Vec<i64> = conn
            .prepare(sql)
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for id in ids {
            rows.push(export(conn, LIBRARY, kind, &id.to_string(), 0).unwrap().unwrap());
        }
    }
    // The account's own playlists and their items, as the account feed carries them.
    let own: Vec<i64> = conn
        .prepare("SELECT id FROM playlists WHERE user_id = ?1 AND library_id = ?2 ORDER BY id")
        .unwrap()
        .query_map([USER, LIBRARY], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for id in own {
        rows.push(export(conn, LIBRARY, Kind::Playlist, &id.to_string(), 0).unwrap().unwrap());
        let keys: Vec<i64> = conn
            .prepare("SELECT key FROM playlist_items WHERE playlist_id = ?1 ORDER BY key")
            .unwrap()
            .query_map([id], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for key in keys {
            let rows_of = export(conn, LIBRARY, Kind::PlaylistItem, &id.to_string(), key);
            rows.push(rows_of.unwrap().unwrap());
        }
    }
    // And the tracks it hid, which it alone reads.
    let hidden: Vec<i64> = conn
        .prepare("SELECT track_id FROM hidden_tracks WHERE user_id = ?1 AND library_id = ?2")
        .unwrap()
        .query_map([USER, LIBRARY], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for id in hidden {
        let rows_of = export_for(conn, USER, LIBRARY, Kind::HiddenTrack, &id.to_string(), 0);
        rows.push(rows_of.unwrap().unwrap());
    }
    // And its recent searches, which it alone reads too.
    let searches: Vec<i64> = conn
        .prepare(
            "SELECT id FROM recent_searches WHERE user_id = ?1 AND library_id = ?2 ORDER BY id",
        )
        .unwrap()
        .query_map([USER, LIBRARY], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for id in searches {
        let rows_of = export_for(conn, USER, LIBRARY, Kind::RecentSearch, &id.to_string(), 0);
        rows.push(rows_of.unwrap().unwrap());
    }
    rows
}

/// Whether `kind` is of the account's own rows, which the account feed carries.
fn account(kind: Kind) -> bool {
    matches!(kind, Kind::Playlist | Kind::PlaylistItem | Kind::HiddenTrack | Kind::RecentSearch)
}

/// A list's query, its sorts, the filters each sort is read with, and the filters it is also
/// walked with in pages of two.
struct List {
    name: &'static str,
    sorts: &'static [&'static str],
    every: Vec<Value>,
    walked: Vec<Value>,
}

/// The filters `listTracks` is read with: none, an album (with tracks that are unknown, or missing
/// or elsewhere), an artist each way they are credited, and tags each way they combine.
fn filters() -> Vec<Value> {
    vec![
        json!({}),
        json!({ "albumId": "101" }),
        json!({ "albumId": "102" }),
        json!({ "albumId": "104" }),
        json!({ "albumId": "201" }),
        json!({ "artistId": "10" }),
        json!({ "artistId": "10", "artistCredit": "owned" }),
        json!({ "artistId": "10", "artistCredit": "featured" }),
        json!({ "artistId": "11" }),
        json!({ "artistId": "13" }),
        json!({ "tagAll": "30" }),
        json!({ "tagAll": "30,32" }),
        json!({ "tagAny": "30,31" }),
        json!({ "tagAll": "30", "tagAny": "32" }),
        json!({ "artistId": "10", "tagAll": "30" }),
        json!({ "tagAll": "999" }),
    ]
}

/// Asks the server's database and the device's `name` with `params`, asserts they answer alike,
/// and returns the answer.
async fn ask(db: &Database, device: &Connection, name: &'static str, params: Value) -> Value {
    let on_server = db
        .read({
            let params = params.clone();
            move |conn| Ok(answer(conn, name, &params))
        })
        .await
        .unwrap();
    assert_eq!(answer(device, name, &params), on_server, "{name} {params}");
    on_server
}

/// The filters `listAlbums` is read with: none, each artist each way they are credited (one of
/// them unknown, one in another library), and tags each way they combine.
fn album_filters() -> Vec<Value> {
    vec![
        json!({}),
        json!({ "artistId": "10" }),
        json!({ "artistId": "10", "artistCredit": "owned" }),
        json!({ "artistId": "10", "artistCredit": "featured" }),
        json!({ "artistId": "11" }),
        json!({ "artistId": "11", "artistCredit": "featured" }),
        json!({ "artistId": "13" }),
        json!({ "artistId": "999" }),
        json!({ "artistId": "20" }),
        json!({ "tagAll": "30" }),
        json!({ "tagAll": "30,32" }),
        json!({ "tagAny": "30,31" }),
        json!({ "tagAny": "31" }),
        json!({ "tagAll": "30", "tagAny": "32" }),
        json!({ "artistId": "10", "tagAll": "30" }),
        json!({ "artistId": "10", "artistCredit": "featured", "tagAny": "30,32" }),
        json!({ "tagAll": "999" }),
    ]
}

/// The filters `listArtists` is read with: none, and tags each way they combine.
fn artist_filters() -> Vec<Value> {
    vec![
        json!({}),
        json!({ "tagAll": "30" }),
        json!({ "tagAll": "30,32" }),
        json!({ "tagAny": "30,31" }),
        json!({ "tagAny": "31" }),
        json!({ "tagAll": "30", "tagAny": "32" }),
        json!({ "tagAll": "999" }),
    ]
}

/// The filters `listTags` is read with: every type, each type, a type with no tag, and names
/// that match by case, by a part and not at all.
fn tag_filters() -> Vec<Value> {
    vec![
        json!({}),
        json!({ "type": "genre" }),
        json!({ "type": "mood" }),
        json!({ "type": "era" }),
        json!({ "q": "ROC" }),
        json!({ "q": "o" }),
        json!({ "q": "   " }),
        json!({ "q": "zzz" }),
        json!({ "type": "genre", "q": "p" }),
    ]
}

/// The filters `listPlaylists` is read with: none, and names that match by case, by a word, by
/// words that are not adjacent and not at all.
fn playlist_filters() -> Vec<Value> {
    vec![
        json!({}),
        json!({ "q": "ROAD" }),
        json!({ "q": "trip road" }),
        json!({ "q": "   " }),
        json!({ "q": "zzz" }),
    ]
}

/// The queries `search` is asked: a track, an album and an artist by name and by a part of it, a
/// word several share, a name the other library has, none that matches, each way a section may
/// be left out, and a small section limit.
fn searches() -> Vec<Value> {
    let mut searches = Vec::new();
    for q in [
        "come",
        "road",
        "radio",
        "a",
        "the beatles",
        "kid a",
        "elsewhere",
        "untitled",
        "zzz",
        "idioteque",
    ] {
        searches.push(json!({ "q": q, "sectionLimit": 5 }));
    }
    for q in ["a", "e"] {
        for types in ["tracks", "albums,artists", "playlists", "artists,tracks"] {
            searches.push(json!({ "q": q, "types": types, "sectionLimit": 1 }));
        }
    }
    searches
}

/// The recipes `resolveRecipe` is asked, each with every sort and direction where it has one, and
/// with each way tags filter: every kind a recipe has, over the library, an album (one of tracks
/// that are missing, one unknown, one elsewhere), an artist each way it is credited, a search and
/// a playlist (one that is not the account's, one elsewhere, and one unknown).
fn recipes() -> Vec<Value> {
    let tag_filters = [
        json!({}),
        json!({ "tagAll": ["30"] }),
        json!({ "tagAny": ["30", "31"] }),
        json!({ "tagAll": ["32"], "tagAny": ["30", "31"] }),
        json!({ "tagAll": ["999"] }),
    ];
    let mut recipes = Vec::new();
    for filters in &tag_filters {
        for sort in SORTS {
            for order in ["asc", "desc"] {
                let by = json!({ "sort": sort, "order": order });
                let mut with = |recipe: Value| {
                    let mut recipe = recipe;
                    recipe.as_object_mut().unwrap().extend(by.as_object().unwrap().clone());
                    recipe["filters"] = filters.clone();
                    recipes.push(recipe);
                };
                with(json!({ "type": "library" }));
                for album in ["101", "102", "104", "201", "999"] {
                    with(json!({ "type": "album", "albumId": album }));
                }
                for (artist, credit) in [
                    ("10", "any"),
                    ("10", "owned"),
                    ("10", "featured"),
                    ("13", "any"),
                    ("20", "any"),
                ] {
                    with(json!({
                        "type": "artist", "artistId": artist, "scope": "all", "artistCredit": credit,
                    }));
                }
            }
        }
        for q in ["a", "e", "come", "zzz"] {
            recipes.push(json!({ "type": "search", "q": q, "filters": filters }));
        }
        for playlist in ["501", "502", "503", "504", "999", "x"] {
            recipes.push(json!({ "type": "playlist", "playlistId": playlist, "filters": filters }));
            for sort in ["name", "artist", "album", "addedToPlaylist", "releaseDate", "duration"] {
                for order in ["asc", "desc"] {
                    recipes.push(json!({
                        "type": "playlist", "playlistId": playlist, "filters": filters,
                        "sort": sort, "order": order,
                    }));
                }
            }
        }
    }
    recipes
}

/// The sorts that lists and recipes are asked with.
const SORTS: [&str; 6] = ["name", "artist", "album", "dateAdded", "releaseDate", "duration"];

/// Each case of the golden file: the query and its answer.
fn case(name: &str, params: Value, answer: Value) -> Value {
    json!({ "query": name, "params": params, "answer": answer })
}

/// A file of `cases`, one to a line so a change reads as a diff.
fn lines(cases: &[Value]) -> String {
    let each: Vec<String> = cases.iter().map(|c| serde_json::to_string(c).unwrap()).collect();
    format!("[\n{}\n]\n", each.join(",\n"))
}

/// The file `name` in `crates/catalog/parity/`, which `UPDATE_PARITY=1` writes from `text`.
fn golden(name: &str, text: &str) {
    golden_bytes(name, text.as_bytes());
}

/// [`golden`] for a file that is not text.
fn golden_bytes(name: &str, bytes: &[u8]) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../catalog/parity").join(name);
    if std::env::var_os("UPDATE_PARITY").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
    }
    let kept = std::fs::read(&path).unwrap_or_default();
    assert!(kept == bytes, "{name} is out of date: run with UPDATE_PARITY=1");
}

/// The library's rows as a snapshot file, which holds none of the account's playlists, written
/// as the server writes one: the database file itself, closed.
fn snapshot(rows: &[Rows]) -> Vec<u8> {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("device.db");
    let conn = Connection::open(&path).unwrap();
    create_schema(&conn).unwrap();
    for rows in rows.iter().filter(|r| !account(r.kind)) {
        apply(&conn, rows).unwrap();
    }
    conn.close().unwrap();
    std::fs::read(path).unwrap()
}

/// The catalog of `testing::catalog` with the account's own rows added: tags of two plugin types,
/// the account's playlists, hidden tracks and Liked Songs, and what it must not see (another
/// account's, and another library's).
async fn fixture() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = catalog::app().await;
    // Tags of two plugin types, which name them: one a library of the device uses, and one only
    // the other library uses. The other library's genre tag is never on the device either.
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO tag_types (key, name, plugin_id, field, since) VALUES \
                 ('decade', 'Decade', 'p', 'DECADE', 1), ('venue', 'Venue', 'p', 'VENUE', 1); \
             INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES \
                 (33, 2, 'genre', 'Elsewhere', 'elsewhere', CAST('elsewhere' AS BLOB)), \
                 (34, 1, 'decade', '1960s', '1960s', CAST('1960s' AS BLOB)), \
                 (35, 2, 'venue', 'Arena', 'arena', CAST('arena' AS BLOB));",
        )?;
        // The account's playlists, one pinned and holding a duplicate and a missing track, one
        // with artwork; and the ones it must not see: another account's, and one in library 2.
        User::new(2, "user").insert(tx)?;
        tx.execute_batch(
            "INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, description, \
                 pinned, version, created_at, modified_at, track_count, duration_us, image_key) \
             VALUES (501, 1, 1, 'Road Trip', 'road trip', CAST('road trip' AS BLOB), 'Driving', 1, \
                         3, 10, 40, 4, 1000, NULL), \
                    (502, 1, 1, 'Quiet', 'quiet', CAST('quiet' AS BLOB), NULL, 0, 1, 20, 30, 2, \
                         500, '40'), \
                    (503, 1, 2, 'Theirs', 'theirs', CAST('theirs' AS BLOB), NULL, 0, 1, 1, 1, 1, \
                         1, NULL), \
                    (504, 2, 1, 'Road Elsewhere', 'road elsewhere', CAST('road elsewhere' AS BLOB), \
                         NULL, 0, 1, 1, 1, 1, 1, NULL); \
             INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) VALUES \
                 (501, 100, 1, 1001, 5000), (501, 200, 1, 1002, 4000), (501, 300, 1, 1001, 3000), \
                 (501, 400, 1, 1004, 2000), (502, 100, 1, 1003, 1000), (502, 200, 1, 1006, 6000), \
                 (503, 100, 1, 1001, 1), (504, 100, 2, 2001, 1);",
        )?;
        // What the account hid: "Bonus", which Radiohead features on and Road Trip holds, and
        // "untitled", the whole of the unknown album and of the unknown artist. Another account
        // hid "Her Majesty", which the account likes, so it shows here as liked and nowhere hidden.
        // The account's Liked Songs, made with the library, under a fixed ID and times so the
        // golden files do not change from run to run.
        tx.execute_batch(
            "UPDATE playlists SET id = 500, created_at = 0, modified_at = 0 \
             WHERE user_id = 1 AND library_id = 1 AND kind = 'liked'; \
             INSERT INTO hidden_tracks (user_id, library_id, track_id, hidden_at) VALUES \
                 (1, 1, 1004, 10), (1, 1, 1006, 20), (2, 1, 1003, 30); \
             INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
             SELECT id, 100, 1, 1003, 7000 FROM playlists \
             WHERE user_id = 1 AND library_id = 1 AND kind = 'liked'; \
             UPDATE playlists SET track_count = 1 \
             WHERE user_id = 1 AND library_id = 1 AND kind = 'liked';",
        )?;
        // The account's recent searches, one that acted on Abbey Road; and the ones it must not
        // see: another account's, and one in library 2.
        tx.execute_batch(
            "INSERT INTO recent_searches (id, user_id, library_id, query, folded, selected_type, \
                 selected_id, searched_at) VALUES \
                 (601, 1, 1, 'Abbey', 'abbey', 'album', 101, 100), \
                 (602, 1, 1, 'Signal', 'signal', NULL, NULL, 200), \
                 (603, 2, 1, 'Theirs', 'theirs', NULL, NULL, 300), \
                 (604, 1, 2, 'Elsewhere', 'elsewhere', NULL, NULL, 400);",
        )
    })
    .await
    .unwrap();
    (temp, db, app)
}

#[tokio::test]
async fn every_list_sort_and_filter_and_every_get_give_identical_json_on_the_server_and_a_synced_device_database()
 {
    let (_temp, db, _app) = fixture().await;
    let (rows, ids) = db
        .read(|conn| {
            let mut ids = HashMap::new();
            for table in ["tracks", "albums", "artists", "tags"] {
                let found: Vec<i64> = conn
                    .prepare(&format!("SELECT id FROM {table} WHERE library_id = 1 ORDER BY id"))?
                    .query_map([], |row| row.get(0))?
                    .collect::<Result<_, _>>()?;
                ids.insert(table, found);
            }
            Ok((feed(conn), ids))
        })
        .await
        .unwrap();
    let entities = |table: &str| ids[table].clone();
    // Through JSON, as the change feed carries the rows.
    let rows: Vec<Rows> = serde_json::from_str(&serde_json::to_string(&rows).unwrap()).unwrap();
    let device = Connection::open_in_memory().unwrap();
    create_schema(&device).unwrap();
    for rows in &rows {
        apply(&device, rows).unwrap();
    }

    // Each list is asked with every sort, direction and filter, then walked in pages of two by
    // its cursors for each sort's own keyset and a few filters' leads.
    let lists = [
        List {
            name: "listTracks",
            sorts: &SORTS,
            every: filters(),
            walked: vec![json!({}), json!({ "artistId": "10" }), json!({ "tagAll": "30,32" })],
        },
        List {
            name: "listAlbums",
            sorts: &["name", "artist", "dateAdded", "releaseDate", "duration"],
            every: album_filters(),
            walked: vec![json!({}), json!({ "artistId": "10" }), json!({ "tagAny": "30,31" })],
        },
        List {
            name: "listArtists",
            sorts: &["name", "dateAdded"],
            every: artist_filters(),
            walked: vec![json!({}), json!({ "tagAny": "30,31" })],
        },
        List {
            name: "listPlaylists",
            sorts: &["name", "created", "modified"],
            every: playlist_filters(),
            walked: vec![json!({}), json!({ "q": "r" })],
        },
        List {
            name: "listTags",
            sorts: &["name", "trackCount"],
            every: tag_filters(),
            walked: vec![json!({}), json!({ "type": "genre" })],
        },
    ];
    let mut cases = Vec::new();
    for List { name, sorts, every, walked } in &lists {
        for sort in sorts.iter() {
            for order in ["asc", "desc"] {
                for filter in every {
                    let mut params = json!({ "sort": sort, "order": order, "limit": 100 });
                    params.as_object_mut().unwrap().extend(filter.as_object().unwrap().clone());
                    let answer = ask(&db, &device, name, params.clone()).await;
                    cases.push(case(name, params, answer));
                }
            }
        }
        for sort in sorts.iter() {
            for order in ["asc", "desc"] {
                for filter in walked {
                    let mut next: Option<String> = None;
                    loop {
                        let mut params = json!({ "sort": sort, "order": order, "limit": 2 });
                        params.as_object_mut().unwrap().extend(filter.as_object().unwrap().clone());
                        params["cursor"] = json!(next);
                        let answer = ask(&db, &device, name, params.clone()).await;
                        next = answer["nextCursor"].as_str().map(str::to_owned);
                        cases.push(case(name, params, answer));
                        if next.is_none() {
                            break;
                        }
                    }
                }
            }
        }
    }
    // Unknown, malformed, repeated and other-library IDs, in an order of their own.
    for named in ["1006,1001", "1003,999,1001,1003,abc,,01", "2001,1004", "999", ""] {
        let params = json!({ "ids": named });
        let answer = ask(&db, &device, "listTracksByIds", params.clone()).await;
        cases.push(case("listTracksByIds", params, answer));
    }
    // Each entity by ID, and an ID that is unknown or belongs to another library.
    let by_id = [
        ("getTrack", "trackId", entities("tracks"), vec![999, 2001]),
        ("getAlbum", "albumId", entities("albums"), vec![999, 201]),
        ("getArtist", "artistId", entities("artists"), vec![999, 20]),
        ("getTag", "tagId", entities("tags"), vec![999, 33, 35]),
    ];
    for (name, key, known, unknown) in by_id {
        for id in known.into_iter().chain(unknown) {
            let params = json!({ key: id.to_string() });
            let answer = ask(&db, &device, name, params.clone()).await;
            cases.push(case(name, params, answer));
        }
    }
    let answer = ask(&db, &device, "listTagTypes", json!({})).await;
    assert_eq!(answer.as_array().unwrap().len(), 3, "only the types the library uses");
    cases.push(case("listTagTypes", json!({}), answer));
    // A playlist by ID, then its items a page at a time and at once: the account's own, another
    // account's, another library's and an unknown one. A cursor from another version of a
    // playlist, and one that is not a cursor, are refused.
    for id in ["501", "502", "503", "504", "999"] {
        let params = json!({ "playlistId": id });
        let answer = ask(&db, &device, "getPlaylist", params.clone()).await;
        cases.push(case("getPlaylist", params, answer));
        for limit in [2, 100] {
            let mut next: Option<String> = None;
            loop {
                let params = json!({ "playlistId": id, "limit": limit, "cursor": next });
                let answer = ask(&db, &device, "listPlaylistItems", params.clone()).await;
                next = answer["nextCursor"].as_str().map(str::to_owned);
                cases.push(case("listPlaylistItems", params, answer));
                if next.is_none() {
                    break;
                }
            }
        }
    }
    let stale = jewelcase_catalog::cursor::encode(
        "playlists.items",
        &[rusqlite::types::Value::Integer(2), rusqlite::types::Value::Integer(100)],
    );
    for cursor in [stale.as_str(), "not a cursor"] {
        let params = json!({ "playlistId": "501", "limit": 2, "cursor": cursor });
        let answer = ask(&db, &device, "listPlaylistItems", params.clone()).await;
        assert!(answer["error"].is_string(), "{answer}");
        cases.push(case("listPlaylistItems", params, answer));
    }
    // Hidden Songs, newest first and oldest first, at once and in pages of one.
    for order in ["desc", "asc"] {
        for limit in [1, 100] {
            let mut next: Option<String> = None;
            loop {
                let params = json!({ "order": order, "limit": limit, "cursor": next });
                let answer = ask(&db, &device, "listHiddenTracks", params.clone()).await;
                next = answer["nextCursor"].as_str().map(str::to_owned);
                cases.push(case("listHiddenTracks", params, answer));
                if next.is_none() {
                    break;
                }
            }
        }
    }
    // The recent searches, newest first.
    let answer = ask(&db, &device, "listRecentSearches", json!({})).await;
    assert_eq!(answer["items"].as_array().map(Vec::len), Some(2), "{answer}");
    cases.push(case("listRecentSearches", json!({}), answer));
    // The documents the search index is built from, and what searches find in it.
    let documents = |conn: &Connection| {
        let mut documents = Vec::new();
        search::documents(conn, LIBRARY, |document| documents.push(format!("{document:?}")))
            .unwrap();
        documents.sort();
        documents
    };
    let on_server = db.read(move |conn| Ok(documents(conn))).await.unwrap();
    assert_eq!(documents(&device), on_server, "the documents of the library");
    assert_eq!(on_server.len(), 6 + 3 + 3, "the library's tracks, its named albums and artists");
    for limit in [1, 4, 100] {
        let (mut batched, mut reading) = (Vec::new(), search::Reading::default());
        let mut add = |document| batched.push(format!("{document:?}"));
        while !search::documents_from(&device, LIBRARY, &mut reading, limit, &mut add).unwrap() {}
        batched.sort();
        assert_eq!(batched, on_server, "the documents read {limit} at a time");
    }
    for params in searches() {
        let answer = ask(&db, &device, "search", params.clone()).await;
        cases.push(case("search", params, answer));
    }
    // Every recipe's tracks, and the list hash they are named by.
    for recipe in recipes() {
        let params = json!({ "recipe": recipe });
        let answer = ask(&db, &device, "resolveRecipe", params.clone()).await;
        cases.push(case("resolveRecipe", params, answer));
    }
    assert!(cases.len() > 200);
    assert!(
        cases.iter().any(|c| c["answer"]["items"].as_array().is_some_and(|i| !i.is_empty())),
        "the fixture yields tracks"
    );

    let feed: Vec<Value> = rows.iter().map(|r| serde_json::to_value(r).unwrap()).collect();
    golden("library.json", &lines(&feed));
    golden_bytes("library.db", &snapshot(&rows));
    // Each file holds the queries of one part, so a change reads as a diff of that part.
    let query = |c: &Value| c["query"].as_str().unwrap().to_owned();
    let (tracks, rest): (Vec<_>, Vec<_>) = cases.into_iter().partition(|c| {
        query(c).ends_with("Tracks") || query(c) == "getTrack" || query(c) == "listTracksByIds"
    });
    let (playlists, rest): (Vec<_>, Vec<_>) =
        rest.into_iter().partition(|c| query(c).contains("Playlist"));
    let (search, rest): (Vec<_>, Vec<_>) = rest.into_iter().partition(|c| query(c) == "search");
    let (recipes, browse): (Vec<_>, Vec<_>) =
        rest.into_iter().partition(|c| query(c) == "resolveRecipe");
    golden("tracks.json", &lines(&tracks));
    golden("browse.json", &lines(&browse));
    golden("playlists.json", &lines(&playlists));
    golden("search.json", &lines(&search));
    golden("recipes.json", &lines(&recipes));
}

/// The account's own rows of `LIBRARY` (playlists, their items, hidden tracks, recent searches),
/// as the account feed sends them, with the times blanked: the server and a device each read
/// their own clock.
fn account_rows(conn: &Connection) -> Vec<Rows> {
    let mut rows: Vec<Rows> = feed(conn).into_iter().filter(|r| account(r.kind)).collect();
    for table in rows.iter_mut().flat_map(|r| r.tables.values_mut()) {
        for row in table.iter_mut() {
            for time in ["created_at", "modified_at", "added_at", "hidden_at", "searched_at"] {
                if row.contains_key(time) {
                    row.insert(time.to_owned(), json!(0));
                }
            }
        }
    }
    rows
}

/// `track` as an edit's playable.
fn playable(track: i64) -> Value {
    json!({ "type": "track", "id": track.to_string() })
}

/// A request as the API takes it: the method, the path and the body.
type Request = (&'static str, String, Option<Value>);

#[tokio::test]
async fn each_account_write_leaves_a_device_database_with_the_rows_the_server_then_feeds() {
    const TRACKS: &str = "/api/v1/libraries/1/tracks";
    const PLAYLISTS: &str = "/api/v1/libraries/1/playlists";
    const SEARCHES: &str = "/api/v1/libraries/1/recent-searches";
    let ids = |tracks: &[i64]| tracks.iter().map(i64::to_string).collect::<Vec<_>>();
    let edit = |playlist: &str, base: i64, extra: Value| {
        let mut body = json!({ "baseVersion": base });
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        ("PATCH", format!("{PLAYLISTS}/{playlist}"), Some(body))
    };
    // Each write as the API takes it, and as a device is told it: the name, and the arguments
    // with the tracks already expanded.
    let writes: Vec<(Request, &str, Value)> = vec![
        (("PUT", format!("{TRACKS}/1002/like"), None), "like", json!({ "trackId": "1002" })),
        (("PUT", format!("{TRACKS}/1004/like"), None), "like", json!({ "trackId": "1004" })),
        (("PUT", format!("{TRACKS}/1003/like"), None), "like", json!({ "trackId": "1003" })),
        (("DELETE", format!("{TRACKS}/1003/like"), None), "unlike", json!({ "trackId": "1003" })),
        (("DELETE", format!("{TRACKS}/1001/like"), None), "unlike", json!({ "trackId": "1001" })),
        (("PUT", format!("{TRACKS}/1003/hidden"), None), "hide", json!({ "trackId": "1003" })),
        (("PUT", format!("{TRACKS}/1001/hidden"), None), "hide", json!({ "trackId": "1001" })),
        (("DELETE", format!("{TRACKS}/1004/hidden"), None), "unhide", json!({ "trackId": "1004" })),
        (("DELETE", format!("{TRACKS}/1002/hidden"), None), "unhide", json!({ "trackId": "1002" })),
        (
            (
                "POST",
                PLAYLISTS.to_owned(),
                Some(json!({ "id": "9001", "title": " Road ", "description": "Long",
                    "items": [playable(1002), playable(1001), playable(1002)] })),
            ),
            "createPlaylist",
            json!({ "id": "9001", "title": " Road ", "description": "Long",
                "items": ids(&[1002, 1001, 1002]) }),
        ),
        (
            ("POST", PLAYLISTS.to_owned(), Some(json!({ "id": "9002", "title": "Empty" }))),
            "createPlaylist",
            json!({ "id": "9002", "title": "Empty" }),
        ),
        (
            ("DELETE", format!("{PLAYLISTS}/501"), None),
            "deletePlaylist",
            json!({ "playlistId": "501" }),
        ),
        (
            ("DELETE", format!("{PLAYLISTS}/500"), None),
            "deletePlaylist",
            json!({ "playlistId": "500" }),
        ),
        (
            edit("501", 3, json!({ "title": "Renamed", "pinned": false, "description": null })),
            "editPlaylist",
            json!({ "playlistId": "501", "baseVersion": 3, "title": "Renamed", "pinned": false,
                "description": null }),
        ),
        (
            edit(
                "501",
                3,
                json!({ "ops": [
                    { "op": "add", "items": [playable(1002)], "at": 1 },
                    { "op": "remove", "positions": [0, 2] },
                    { "op": "move", "positions": [1], "at": 0 },
                    { "op": "add", "items": [playable(1003)] },
                ] }),
            ),
            "editPlaylist",
            json!({ "playlistId": "501", "baseVersion": 3, "ops": [
                { "op": "add", "items": ["1002"], "at": 1 },
                { "op": "remove", "positions": [0, 2] },
                { "op": "move", "positions": [1], "at": 0 },
                { "op": "add", "items": ["1003"] },
            ] }),
        ),
        (
            edit("505", 1, json!({ "ops": [{ "op": "add", "items": [playable(1002)], "at": 1 }] })),
            "editPlaylist",
            json!({ "playlistId": "505", "baseVersion": 1,
                "ops": [{ "op": "add", "items": ["1002"], "at": 1 }] }),
        ),
        (
            edit(
                "500",
                1,
                json!({ "ops": [{ "op": "add",
                    "items": [playable(1002), playable(1004), playable(1003)] }] }),
            ),
            "editPlaylist",
            json!({ "playlistId": "500", "baseVersion": 1,
                "ops": [{ "op": "add", "items": ["1002", "1004", "1003"] }] }),
        ),
        (
            edit("500", 1, json!({ "ops": [{ "op": "remove", "positions": [0] }] })),
            "editPlaylist",
            json!({ "playlistId": "500", "baseVersion": 1,
                "ops": [{ "op": "remove", "positions": [0] }] }),
        ),
        (
            edit("501", 3, json!({ "ops": [{ "op": "add", "items": [playable(1003)] }] })),
            "appendToPlaylist",
            json!({ "playlistId": "501", "items": ["1003"] }),
        ),
        (
            edit("500", 1, json!({ "ops": [{ "op": "add", "items": [playable(1002)] }] })),
            "appendToPlaylist",
            json!({ "playlistId": "500", "items": ["1002"] }),
        ),
        (
            ("POST", SEARCHES.to_owned(), Some(json!({ "id": "9101", "query": " Help " }))),
            "recordSearch",
            json!({ "id": "9101", "query": " Help " }),
        ),
        (
            (
                "POST",
                SEARCHES.to_owned(),
                Some(json!({ "id": "9102", "query": "SIGNAL",
                    "selected": { "type": "track", "id": "1002" } })),
            ),
            "recordSearch",
            json!({ "id": "9102", "query": "SIGNAL", "selected": { "type": "track", "id": "1002" } }),
        ),
        (("DELETE", format!("{SEARCHES}/601"), None), "deleteSearch", json!({ "id": "601" })),
        (("DELETE", SEARCHES.to_owned(), None), "clearSearches", json!({})),
    ];
    // The likes, unlikes and unhides that find nothing to do leave the rows as they were.
    let (mut unchanged, count) = (0, writes.len());
    for ((method, uri, body), name, args) in writes {
        let (_temp, db, app) = fixture().await;
        // What the fixture leaves out: art on Abbey Road, so a playlist of its tracks has
        // artwork, and a playlist whose keys are adjacent, so an add between them renumbers it.
        db.write(|tx| {
            tx.execute_batch(
                "UPDATE albums SET image_id = 40 WHERE id = 101; \
                 INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, \
                     version, created_at, modified_at, track_count) \
                 VALUES (505, 1, 1, 'Tight', 'tight', CAST('tight' AS BLOB), 1, 1, 1, 3); \
                 INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
                 VALUES (505, 1, 1, 1001, 1), (505, 2, 1, 1002, 1), (505, 3, 1, 1003, 1);",
            )
        })
        .await
        .unwrap();
        let rows = db.read(|conn| Ok(feed(conn))).await.unwrap();
        let device = Connection::open_in_memory().unwrap();
        create_schema(&device).unwrap();
        for rows in &rows {
            apply(&device, rows).unwrap();
        }
        let before = account_rows(&device);
        assert_eq!(before, db.read(|conn| Ok(account_rows(conn))).await.unwrap());

        let (status, answer) = call(&app, TOKEN, method, &uri, body).await;
        assert!(status.is_success(), "{name} {args}: {status} {answer}");
        let undo = mutate::apply(&device, USER, LIBRARY, name, &args.to_string(), 100).unwrap();

        let on_server = db.read(|conn| Ok(account_rows(conn))).await.unwrap();
        unchanged += usize::from(on_server == before);
        assert_eq!(account_rows(&device), on_server, "{name} {args}");
        // The device holds nothing the feed would not send: its tables hold no more rows than
        // the feed's entities, which `account_rows` reads through the playlists that exist.
        for (table, kind) in [
            ("playlists", Kind::Playlist),
            ("playlist_items", Kind::PlaylistItem),
            ("hidden_tracks", Kind::HiddenTrack),
            ("recent_searches", Kind::RecentSearch),
        ] {
            let held: usize = device
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))
                .unwrap();
            let sent = on_server.iter().filter(|rows| rows.kind == kind).count();
            assert_eq!(held, sent, "{name} {args}: the device's {table}");
        }
        // And what it undoes is what the device held before.
        mutate::revert(&device, &undo).unwrap();
        assert_eq!(account_rows(&device), before, "{name} {args} reverted");
    }
    assert_eq!(unchanged, 3, "of {count} writes");
}
