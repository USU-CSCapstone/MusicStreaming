//! The catalog's shared queries give the same JSON on the server's database and on a device
//! database synced from it (`design/general.md` §3). The answers are also written to
//! `crates/catalog/parity/`, with the rows that built the device, for `catalog-wasm` to replay
//! (`UPDATE_PARITY=1` rewrites them).

use std::collections::HashMap;
use std::path::PathBuf;

use jewelcase_catalog::albums::{self, AlbumSort};
use jewelcase_catalog::artists::{self, ArtistSort};
use jewelcase_catalog::sync::{Kind, Rows, apply, export};
use jewelcase_catalog::tags::{self, TagSort};
use jewelcase_catalog::tracks::{self, TrackOrder, TrackSort};
use jewelcase_catalog::{AnyId, Id, create_schema};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::credit::ArtistCredit;
use super::super::page::{Order, Request};
use super::super::tag_filter::TagFilter;
use super::super::testing::catalog;
use crate::db::Database;

const LIBRARY: i64 = 1;

/// A `listTracks` request's parameters, as the API spells them in a query.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Params {
    sort: TrackSort,
    order: Order,
    album_id: Option<AnyId>,
    artist_id: Option<AnyId>,
    artist_credit: Option<ArtistCredit>,
    tag_all: Option<String>,
    tag_any: Option<String>,
    cursor: Option<String>,
    limit: u32,
}

/// A `listAlbums` request's parameters, as the API spells them in a query.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlbumParams {
    sort: AlbumSort,
    order: Order,
    artist_id: Option<AnyId>,
    artist_credit: Option<ArtistCredit>,
    tag_all: Option<String>,
    tag_any: Option<String>,
    cursor: Option<String>,
    limit: u32,
}

/// A `listArtists` request's parameters, as the API spells them in a query.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArtistParams {
    sort: ArtistSort,
    order: Order,
    tag_all: Option<String>,
    tag_any: Option<String>,
    cursor: Option<String>,
    limit: u32,
}

/// A `listTags` request's parameters, as the API spells them in a query.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TagParams {
    sort: TagSort,
    order: Order,
    r#type: Option<String>,
    q: Option<String>,
    cursor: Option<String>,
    limit: u32,
}

/// The ID `params` holds under `key`, as the API's path would carry it.
fn path_id(params: &Value, key: &str) -> i64 {
    Id::canonical(params[key].as_str().unwrap()).unwrap().0
}

/// Answers the query `name` with `params` from `conn`, as the API would send it.
fn answer(conn: &Connection, name: &str, params: &Value) -> Value {
    match name {
        "listTracks" => {
            let p: Params = serde_json::from_value(params.clone()).unwrap();
            let (sort, order) =
                TrackOrder { sort: p.sort, order: p.order }.keyset(p.album_id.is_some());
            let request = Request::new(sort, order, p.cursor, Some(p.limit)).unwrap();
            let tags = TagFilter::new(p.tag_all.as_deref(), p.tag_any.as_deref()).unwrap();
            let page = tracks::list(
                conn,
                LIBRARY,
                request,
                p.album_id,
                p.artist_id,
                p.artist_credit.unwrap_or_default(),
                &tags,
            )
            .unwrap();
            serde_json::to_value(page).unwrap()
        }
        "listTracksByIds" => {
            let ids: Vec<i64> = params["ids"]
                .as_str()
                .unwrap()
                .split(',')
                .filter_map(|id| AnyId::from(id).0)
                .collect();
            serde_json::to_value(tracks::by_ids(conn, LIBRARY, &ids).unwrap()).unwrap()
        }
        "getTrack" => {
            serde_json::to_value(tracks::track(conn, LIBRARY, path_id(params, "trackId")).unwrap())
                .unwrap()
        }
        "listAlbums" => {
            let p: AlbumParams = serde_json::from_value(params.clone()).unwrap();
            let request = Request::new(p.sort.sort(), p.order, p.cursor, Some(p.limit)).unwrap();
            let tags = TagFilter::new(p.tag_all.as_deref(), p.tag_any.as_deref()).unwrap();
            let credit = p.artist_credit.unwrap_or_default();
            let page = albums::list(conn, LIBRARY, request, p.artist_id, credit, &tags).unwrap();
            serde_json::to_value(page).unwrap()
        }
        "getAlbum" => {
            serde_json::to_value(albums::album(conn, LIBRARY, path_id(params, "albumId")).unwrap())
                .unwrap()
        }
        "listArtists" => {
            let p: ArtistParams = serde_json::from_value(params.clone()).unwrap();
            let request = Request::new(p.sort.sort(), p.order, p.cursor, Some(p.limit)).unwrap();
            let tags = TagFilter::new(p.tag_all.as_deref(), p.tag_any.as_deref()).unwrap();
            serde_json::to_value(artists::list(conn, LIBRARY, request, &tags).unwrap()).unwrap()
        }
        "getArtist" => serde_json::to_value(
            artists::artist(conn, LIBRARY, path_id(params, "artistId")).unwrap(),
        )
        .unwrap(),
        "listTagTypes" => serde_json::to_value(tags::types(conn, LIBRARY).unwrap()).unwrap(),
        "listTags" => {
            let p: TagParams = serde_json::from_value(params.clone()).unwrap();
            let request = Request::new(p.sort.sort(), p.order, p.cursor, Some(p.limit)).unwrap();
            let page = tags::list(conn, LIBRARY, request, p.r#type, p.q.as_deref()).unwrap();
            serde_json::to_value(page).unwrap()
        }
        "getTag" => {
            serde_json::to_value(tags::find(conn, LIBRARY, path_id(params, "tagId")).unwrap())
                .unwrap()
        }
        _ => panic!("unknown query {name}"),
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
    rows
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
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../catalog/parity").join(name);
    if std::env::var_os("UPDATE_PARITY").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    let kept = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(kept == text, "{name} is out of date: run with UPDATE_PARITY=1");
}

#[tokio::test]
async fn every_list_sort_and_filter_and_every_get_give_identical_json_on_the_server_and_a_synced_device_database()
 {
    let (_temp, db, _app) = catalog::app().await;
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
        )
    })
    .await
    .unwrap();
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
    assert!(cases.len() > 200);
    assert!(
        cases.iter().any(|c| c["answer"]["items"].as_array().is_some_and(|i| !i.is_empty())),
        "the fixture yields tracks"
    );

    let feed: Vec<Value> = rows.iter().map(|r| serde_json::to_value(r).unwrap()).collect();
    golden("library.json", &lines(&feed));
    let (tracks, rest): (Vec<_>, Vec<_>) = cases.into_iter().partition(|c| {
        c["query"].as_str().unwrap().ends_with("Tracks")
            || c["query"] == "getTrack"
            || c["query"] == "listTracksByIds"
    });
    golden("tracks.json", &lines(&tracks));
    golden("browse.json", &lines(&rest));
}
