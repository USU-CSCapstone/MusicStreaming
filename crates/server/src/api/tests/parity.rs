//! The catalog's shared queries give the same JSON on the server's database and on a device
//! database synced from it (`design/general.md` §3). The answers are also written to
//! `crates/catalog/parity/`, with the rows that built the device, for `catalog-wasm` to replay
//! (`UPDATE_PARITY=1` rewrites them).

use std::path::PathBuf;

use jewelcase_catalog::sync::{Kind, Rows, apply, export};
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
            let id = Id::canonical(params["trackId"].as_str().unwrap()).unwrap();
            serde_json::to_value(tracks::track(conn, LIBRARY, id.0).unwrap()).unwrap()
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

/// The filters every sort is read with: none, an album (with tracks that are unknown, or missing
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
async fn every_track_sort_and_filter_gives_identical_json_on_the_server_and_a_synced_device_database()
 {
    let (_temp, db, _app) = catalog::app().await;
    let (rows, ids) = db
        .read(|conn| {
            let ids: Vec<i64> = conn
                .prepare("SELECT id FROM tracks WHERE library_id = 1 ORDER BY id")?
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            Ok((feed(conn), ids))
        })
        .await
        .unwrap();
    // Through JSON, as the change feed carries the rows.
    let rows: Vec<Rows> = serde_json::from_str(&serde_json::to_string(&rows).unwrap()).unwrap();
    let device = Connection::open_in_memory().unwrap();
    create_schema(&device).unwrap();
    for rows in &rows {
        apply(&device, rows).unwrap();
    }

    // Each query is asked of the server's database and of the device's.
    let mut queries: Vec<(&str, Value)> = Vec::new();
    for sort in SORTS {
        for order in ["asc", "desc"] {
            for filter in filters() {
                let mut params = json!({ "sort": sort, "order": order, "limit": 100 });
                params.as_object_mut().unwrap().extend(filter.as_object().unwrap().clone());
                queries.push(("listTracks", params));
            }
        }
    }
    let mut cases = Vec::new();
    for (name, params) in queries {
        let answer = ask(&db, &device, name, params.clone()).await;
        cases.push(case(name, params, answer));
    }
    // Pages of two, walked to the end by their cursors, for the sorts' own keysets, an artist's
    // lead and a tag's.
    for sort in SORTS {
        for order in ["asc", "desc"] {
            for filter in [json!({}), json!({ "artistId": "10" }), json!({ "tagAll": "30,32" })] {
                let mut next: Option<String> = None;
                loop {
                    let mut params = json!({ "sort": sort, "order": order, "limit": 2 });
                    params.as_object_mut().unwrap().extend(filter.as_object().unwrap().clone());
                    params["cursor"] = json!(next);
                    let answer = ask(&db, &device, "listTracks", params.clone()).await;
                    next = answer["nextCursor"].as_str().map(str::to_owned);
                    cases.push(case("listTracks", params, answer));
                    if next.is_none() {
                        break;
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
    for id in ids.iter().copied().chain([999]) {
        let params = json!({ "trackId": id.to_string() });
        let answer = ask(&db, &device, "getTrack", params.clone()).await;
        cases.push(case("getTrack", params, answer));
    }
    assert!(cases.len() > 200);
    assert!(
        cases.iter().any(|c| c["answer"]["items"].as_array().is_some_and(|i| !i.is_empty())),
        "the fixture yields tracks"
    );

    let feed: Vec<Value> = rows.iter().map(|r| serde_json::to_value(r).unwrap()).collect();
    golden("library.json", &lines(&feed));
    golden("tracks.json", &lines(&cases));
}
