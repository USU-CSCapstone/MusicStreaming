//! Starting from a recipe, and the recipes resolving as the views they stand for page.

use axum::Router;
use axum::http::StatusCode;
use jewelcase_core::{SortField, SortRow, sort_rows};
use serde_json::{Value, json};

use super::{KEY, KEY2, OTHER, QUEUE, snapshot, start};
use crate::api::queue::{Recipe, resolve};
use crate::api::scope::Caller;
use crate::api::testing::catalog::page_through;
use crate::api::testing::{TOKEN, add_device, add_user, app, call, conn, json, router_over, state};
use crate::api::{AppState, Id};
use crate::db::testing::{
    Album, Artist, Track, access, album_artist, library, scattered_key, track_artist,
};
use crate::queue::list_hash;

/// A track of library 1, as `fixture` adds it: its ID, album, title, artist key, disc, number,
/// release date and duration, and the milliseconds it was added at.
type Row = (i64, i64, &'static str, &'static str, i64, Option<i64>, Option<&'static str>, i64, i64);

/// Library 1: two artists, three albums, and 14 tracks that tie in each sort at least once and
/// lack each sort's value at least once (no artist, no album name, no date, no number); library
/// 2 holds track 900, which user 2 does not reach.
const TRACKS: [Row; 14] = [
    (1, 101, "Song", "alpha", 1, Some(1), Some("2000"), 200, 1),
    (2, 101, "Song", "alpha", 1, Some(2), Some("2000"), 200, 1),
    (3, 101, "Tune", "alpha", 1, None, Some("2000"), 300, 2),
    (4, 101, "Tune", "alpha", 2, Some(1), Some("2000"), 300, 2),
    (5, 102, "Aria", "beta", 1, Some(1), None, 100, 3),
    (6, 102, "Aria", "beta", 1, Some(1), None, 100, 3),
    (7, 102, "Hymn", "", 1, Some(2), None, 250, 4),
    (8, 102, "Hymn", "", 0, None, Some("1999-05"), 250, 4),
    (9, 103, "Ode", "", 1, Some(1), None, 100, 5),
    (10, 103, "", "", 1, Some(1), None, 100, 5),
    (11, 103, "", "beta", 1, Some(2), Some("2000"), 150, 5),
    (12, 101, "Song", "alpha", 1, Some(1), Some("2000"), 200, 1),
    (13, 102, "Aria", "beta", 1, Some(1), Some("1999-05"), 300, 3),
    (14, 103, "Hymn", "", 0, None, None, 100, 4),
];

/// The API over `TRACKS`, with the owner as `TOKEN` and user 2, who reaches only library 1, as
/// `user2`. Artist 10 owns album 101 and is on tracks 1 to 3, 6 and 13; artist 11 owns album 102
/// and is on tracks 4 to 6, 11 and 13. Tags 30 to 32 are on library 1's tracks, and tag 50 on
/// library 2's.
pub(super) async fn fixture() -> (tempfile::TempDir, AppState, Router) {
    let (temp, db, _app) = app("");
    db.write(|tx| {
        library(tx, 1, "Music", "/music")?;
        library(tx, 2, "Other", "/other")?;
        Artist::new(10, 1, "Alpha").insert(tx)?;
        Artist::new(11, 1, "Beta").insert(tx)?;
        for (album, title, artist) in [(101, "Album", 10), (102, "Disc", 11)] {
            Album { title: Some(title.into()), ..Album::new(album, 1) }.insert(tx)?;
            album_artist(tx, 1, album, artist)?;
        }
        Album::new(103, 1).insert(tx)?;
        Album::new(201, 2).insert(tx)?;
        for (id, album, title, artist, disc, number, release, duration, added) in TRACKS {
            Track {
                path: format!("p{id}"),
                title: title.into(),
                sort_key: title.to_lowercase().into_bytes(),
                artist_sort_key: artist.as_bytes().to_vec(),
                album_sort_key: if album == 103 { Vec::new() } else { scattered_key("a", album) },
                disc_number: disc,
                track_number: number,
                release_date: release.map(str::to_owned),
                duration_us: duration,
                added_at: added,
                ..Track::new(id, 1, album)
            }
            .insert(tx)?;
        }
        Track::new(900, 2, 201).insert(tx)?;
        // Rock (30) is on every third track, Jazz (31) on every fifth, Calm (32) on every even
        // one, and library 2's Rock (50) on its track.
        tx.execute_batch(
            "INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES
                 (30, 1, 'genre', 'Rock', 'rock', x'72'), (31, 1, 'genre', 'Jazz', 'jazz', x'6a'),
                 (32, 1, 'mood', 'Calm', 'calm', x'63'), (50, 2, 'genre', 'Rock', 'rock', x'72');
             INSERT INTO track_tags (library_id, track_id, tag_id, tag_name)
             SELECT 1, id, 30, 'Rock' FROM tracks WHERE library_id = 1 AND id % 3 = 0
             UNION ALL SELECT 1, id, 31, 'Jazz' FROM tracks WHERE library_id = 1 AND id % 5 = 0
             UNION ALL SELECT 1, id, 32, 'Calm' FROM tracks WHERE library_id = 1 AND id % 2 = 0
             UNION ALL SELECT 2, 900, 50, 'Rock';
             UPDATE tags SET track_count = (SELECT count(*) FROM track_tags WHERE tag_id = tags.id);",
        )?;
        for (track, position, artist) in
            [(1, 0, 10), (2, 0, 10), (3, 0, 10), (6, 1, 10), (13, 1, 10)]
        {
            track_artist(tx, 1, track, position, artist, "Alpha")?;
        }
        for (track, artist) in [(4, 11), (5, 11), (6, 11), (11, 11), (13, 11)] {
            track_artist(tx, 1, track, 0, artist, "Beta")?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 3, "user2");
    access(&conn, 2, 1).unwrap();
    let state = state(&temp, db);
    let app = router_over(state.clone());
    (temp, state, app)
}

/// The tracks `recipe` resolves to in library 1 for the owner, as the API spells their IDs.
pub(super) async fn resolved(state: &AppState, recipe: &Value) -> Vec<String> {
    let caller = Caller { user: 1, device: 1, admin: true };
    let recipe: Recipe = serde_json::from_value(recipe.clone()).unwrap();
    let (ids, _) = resolve(&state.db, &state.indexes, &caller, Id(1), &recipe).await.unwrap();
    ids.iter().map(u64::to_string).collect()
}

/// The body of a start whose source is `recipe` in library 1 under `key`, playing `track` first,
/// shown at index `start`.
pub(super) fn recipe_start(
    queue: &str,
    key: &str,
    recipe: Value,
    track: &str,
    start: u64,
) -> Value {
    json!({
        "queueId": queue, "context": { "type": "library" }, "start": start,
        "startTrackId": track, "source": { "key": key, "libraryId": "1", "recipe": recipe },
    })
}

const SORTS: [&str; 6] = ["name", "artist", "album", "dateAdded", "releaseDate", "duration"];

/// Tag filters a recipe carries and the query a view sends for them: none, all of one genre, any
/// of two genres, and one mood with any of two genres.
const FILTERS: [(&str, &str); 4] = [
    ("{}", ""),
    (r#"{"tagAll":["30"]}"#, "&tagAll=30"),
    (r#"{"tagAny":["30","31"]}"#, "&tagAny=30,31"),
    (r#"{"tagAll":["32"],"tagAny":["30","31"]}"#, "&tagAll=32&tagAny=30,31"),
];

#[tokio::test]
async fn paged_view_equals_resolve() {
    let (_temp, state, app) = fixture().await;
    let mut checked = 0;
    for sort in SORTS {
        for order in ["asc", "desc"] {
            for (filters, tags) in FILTERS {
                let filters: Value = serde_json::from_str(filters).unwrap();
                // An album and artist recipe with no filter leaves the field out.
                let with = |mut recipe: Value| {
                    if filters.as_object().is_some_and(|filters| !filters.is_empty()) {
                        recipe["filters"] = filters.clone();
                    }
                    recipe
                };
                let views = [
                    (
                        with(
                            json!({ "type": "album", "albumId": "101", "sort": sort, "order": order }),
                        ),
                        format!("albumId=101&sort={sort}&order={order}{tags}"),
                    ),
                    (
                        with(
                            json!({ "type": "album", "albumId": "103", "sort": sort, "order": order }),
                        ),
                        format!("albumId=103&sort={sort}&order={order}{tags}"),
                    ),
                    (
                        json!({ "type": "library", "filters": filters, "sort": sort, "order": order }),
                        format!("sort={sort}&order={order}{tags}"),
                    ),
                ];
                let credits = ["any", "owned", "featured"].into_iter().flat_map(|credit| {
                    ["10", "11", "99"].map(|artist| {
                        (
                            with(json!({
                                "type": "artist", "artistId": artist, "scope": "all",
                                "artistCredit": credit, "sort": sort, "order": order,
                            })),
                            format!(
                                "artistId={artist}&artistCredit={credit}&sort={sort}&order={order}{tags}"
                            ),
                        )
                    })
                });
                for (recipe, query) in views.into_iter().chain(credits) {
                    let ids = resolved(&state, &recipe).await;
                    for limit in [1, 2, 3, 100] {
                        let uri = format!("/api/v1/libraries/1/tracks?{query}");
                        let paged = page_through(&app, &uri, limit, "id").await;
                        assert_eq!(json!(ids), paged, "{recipe}, limit {limit}");
                    }
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 12 * 12 * FILTERS.len());

    // Ties end on the ID, and a track with no name sorts last in both directions.
    let name = |order| json!({ "type": "library", "filters": {}, "sort": "name", "order": order });
    let up = ["5", "6", "13", "7", "8", "14", "9", "1", "2", "12", "3", "4", "10", "11"];
    let down = ["4", "3", "12", "2", "1", "9", "14", "8", "7", "13", "6", "5", "11", "10"];
    assert_eq!(resolved(&state, &name("asc")).await, up);
    assert_eq!(resolved(&state, &name("desc")).await, down);
}

/// An album page sorts its own tracks by the core's comparator over the keys the API gives it,
/// while the album recipe resolves in SQL: for every sort the page offers, in both directions
/// and under each filter, the two agree (`requirements/conventions.md` §4.1).
#[tokio::test]
async fn an_album_recipe_resolves_the_rows_its_page_sorts() {
    let (_temp, state, app) = fixture().await;
    for album in ["101", "102", "103"] {
        for (filters, query) in FILTERS {
            let uri =
                format!("/api/v1/libraries/1/tracks?albumId={album}&sort=album&limit=100{query}");
            let (_, page) = json(&app, &uri).await;
            let rows = page["items"].as_array().unwrap();
            let keys: Vec<SortRow> = rows
                .iter()
                .map(|t| {
                    serde_json::from_value(json!({
                        "id": t["id"], "titleKey": t["sortKeys"]["title"],
                        "artistKey": t["sortKeys"]["artist"], "albumKey": t["sortKeys"]["album"],
                        "albumId": t["album"]["id"], "releaseDate": t["releaseDate"],
                        "disc": t["discNumber"], "number": t["trackNumber"],
                        "durationUs": t["durationUs"],
                    }))
                    .unwrap()
                })
                .collect();
            for sort in ["name", "artist", "duration"] {
                for order in ["asc", "desc"] {
                    let field: SortField = serde_json::from_value(json!(sort)).unwrap();
                    let shown: Vec<String> = sort_rows(&keys, field, order == "desc")
                        .into_iter()
                        .map(|at| rows[at]["id"].as_str().unwrap().to_owned())
                        .collect();
                    let mut recipe =
                        json!({ "type": "album", "albumId": album, "sort": sort, "order": order });
                    if !query.is_empty() {
                        recipe["filters"] = serde_json::from_str(filters).unwrap();
                    }
                    assert_eq!(resolved(&state, &recipe).await, shown, "{recipe}");
                }
            }
        }
    }
}

#[tokio::test]
async fn search_resolve_equals_the_search_section() {
    let (_temp, state, app) = fixture().await;
    for q in ["song", "hymn", "aria tune", "o", "nothing"] {
        let ids = resolved(&state, &json!({ "type": "search", "q": q })).await;
        let (status, found) = json(
            &app,
            &format!(
                "/api/v1/libraries/1/search?q={}&types=tracks&sectionLimit=50",
                q.replace(' ', "%20")
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{q}");
        let shown: Vec<String> = found["sections"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|section| section["items"].as_array().unwrap())
            .map(|item| item["track"]["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, shown, "{q}");
    }
    assert_eq!(resolved(&state, &json!({ "type": "search", "q": "song" })).await.len(), 3);
}

#[tokio::test]
async fn tag_filters_narrow_each_recipe() {
    let (_temp, state, app) = fixture().await;
    let library =
        |tags: Value| json!({ "type": "library", "filters": tags, "sort": "name", "order": "asc" });
    // Rock is on tracks 3, 6, 9 and 12; Calm is on the even ones; Jazz on 5 and 10.
    assert_eq!(
        resolved(&state, &library(json!({ "tagAll": ["30"] }))).await,
        ["6", "9", "12", "3"]
    );
    assert_eq!(
        resolved(&state, &library(json!({ "tagAll": ["30"], "tagAny": ["32"] }))).await,
        ["6", "12"]
    );
    assert_eq!(resolved(&state, &library(json!({ "tagAny": ["31", "30"] }))).await.len(), 6);

    // A search narrows as the search endpoint does.
    for (q, filters, query) in [
        ("hymn", json!({ "tagAll": ["32"] }), "tagAll=32"),
        ("o", json!({ "tagAny": ["30", "31"] }), "tagAny=30,31"),
        ("song", json!({ "tagAny": ["30"] }), "tagAny=30"),
        ("song", json!({ "tagAll": ["99"] }), "tagAll=99"),
    ] {
        let ids = resolved(&state, &json!({ "type": "search", "q": q, "filters": filters })).await;
        let uri = format!("/api/v1/libraries/1/search?q={q}&types=tracks&sectionLimit=50&{query}");
        let (status, found) = json(&app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{q}");
        let shown: Vec<String> = found["sections"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|section| section["items"].as_array().unwrap())
            .map(|item| item["track"]["id"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(ids, shown, "{q} {query}");
    }
    let hymn = json!({ "type": "search", "q": "hymn", "filters": { "tagAll": ["32"] } });
    assert_eq!(resolved(&state, &hymn).await, ["8", "14"]);

    // Library 2's tag and track are not found in library 1: a tag of another library names
    // nothing, and a search for track 900's title finds nothing there either.
    for recipe in [
        library(json!({ "tagAll": ["50"] })),
        library(json!({ "tagAny": ["50"] })),
        library(json!({ "tagAny": ["30", "50"] })),
        json!({ "type": "album", "albumId": "101", "filters": { "tagAll": ["50"] }, "sort": "name", "order": "asc" }),
        json!({ "type": "search", "q": "song", "filters": { "tagAny": ["50"] } }),
    ] {
        assert!(resolved(&state, &recipe).await.is_empty(), "{recipe}");
    }
}

#[tokio::test]
async fn a_recipe_keeps_its_tag_filter() {
    let (_temp, _state, app) = fixture().await;
    // Rock and Calm are on tracks 6 and 12.
    let filters = json!({ "tagAll": ["30"], "tagAny": ["32"] });
    let sort = json!({ "sort": "name", "order": "asc" });
    let with_sort = |mut recipe: Value| {
        recipe["sort"] = sort["sort"].clone();
        recipe["order"] = sort["order"].clone();
        recipe
    };
    let mut n = 0;
    let mut start_new = |recipe: Value, track: &str| {
        n += 1;
        let queue = format!("33333333-3333-4333-8333-33333333333{n}");
        let key = format!("44444444-4444-4444-8444-44444444444{n}");
        start(&app, TOKEN, recipe_start(&queue, &key, recipe, track, 0))
    };
    for (recipe, track) in [
        (json!({ "type": "library", "filters": filters }), "6"),
        (json!({ "type": "album", "albumId": "101", "filters": filters }), "12"),
        (
            json!({ "type": "artist", "artistId": "10", "scope": "all", "artistCredit": "any", "filters": filters }),
            "6",
        ),
    ] {
        let recipe = with_sort(recipe);
        let (status, answer) = start_new(recipe.clone(), track).await;
        assert_eq!(status, StatusCode::OK, "{recipe}: {answer}");
        assert_eq!(snapshot(&app).await["state"]["sources"][0]["recipe"], recipe);
    }
    let search = json!({ "type": "search", "q": "song", "filters": filters });
    let (status, answer) = start_new(search.clone(), "12").await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(snapshot(&app).await["state"]["sources"][0]["recipe"], search);

    // More tags than a list takes, and a filter other than tags, are refused.
    let many: Vec<String> = (0..101).map(|_| "30".to_owned()).collect();
    let limit =
        json!({ "type": "library", "filters": { "tagAny": many }, "sort": "name", "order": "asc" });
    // The start tracks carry the tag, so only the limit or the filter refuses them.
    for (what, recipe, track, detail) in [
        ("over MAX_TAGS", limit, "6", "at most 100 tags"),
        (
            "over together",
            json!({ "type": "search", "q": "song", "filters": { "tagAll": &many[..50], "tagAny": &many[..51] } }),
            "12",
            "at most 100 tags",
        ),
        (
            "not a tag filter",
            json!({ "type": "album", "albumId": "101", "filters": { "yearFrom": 1999 }, "sort": "name", "order": "asc" }),
            "1",
            "can only be tags",
        ),
    ] {
        let (status, body) = start(&app, TOKEN, recipe_start(QUEUE, KEY, recipe, track, 0)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{what}: {body}");
        assert!(body["detail"].as_str().unwrap().contains(detail), "{what}: {body}");
    }
}

#[tokio::test]
async fn start_follows_start_track_id() {
    let (_temp, _state, app) = fixture().await;
    let library = json!({ "type": "library", "filters": {}, "sort": "name", "order": "asc" });
    // Name order is 5 6 13 7 8 14 9 1 2 12 3 4 10 11.
    let from = |queue: &str, key: &str, track: &str, start: u64| {
        recipe_start(queue, key, library.clone(), track, start)
    };
    let (status, answer) = start(&app, TOKEN, from(QUEUE, KEY, "9", 6)).await;
    assert_eq!((status, &answer["length"]), (StatusCode::OK, &json!(14)), "{answer}");
    let order = ["5", "6", "13", "7", "8", "14", "9", "1", "2", "12", "3", "4", "10", "11"];
    let list = list_hash(&order.map(|id| id.parse::<u64>().unwrap()));
    assert_eq!(answer["list"], json!(list));
    // Where the device had it: the server uses its own position, and the cursor is after it.
    let queue = snapshot(&app).await;
    assert_eq!(queue["state"]["current"], json!(format!("{KEY}.6")));
    assert_eq!(queue["state"]["cursor"], json!(7));
    assert_eq!(
        queue["state"]["sources"][0]["recipe"],
        json!({ "type": "library", "filters": {}, "sort": "name", "order": "asc" })
    );

    // The library changed since the view loaded: the track is not at `start`, so the first
    // place it appears is where it plays; `start` past the end does the same.
    for (n, start_at) in [0, 100].into_iter().enumerate() {
        let queue = format!("33333333-3333-4333-8333-33333333333{n}");
        let key = format!("44444444-4444-4444-8444-44444444444{n}");
        let (status, answer) = start(&app, TOKEN, from(&queue, &key, "9", start_at)).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_eq!(snapshot(&app).await["state"]["current"], json!(format!("{key}.6")));
    }

    // A track the list lacks, one that is not an ID, and one of another library.
    let (status, _) = start(&app, TOKEN, from(OTHER, KEY2, "99", 0)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "a track the list lacks");
    let (status, _) = start(&app, TOKEN, from(OTHER, KEY2, "nine", 0)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "not an ID");
    let (status, _) = start(&app, TOKEN, from(OTHER, KEY2, "900", 0)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "a track of another library");
}

#[tokio::test]
async fn recipe_refuses_unreachable_library() {
    let (_temp, _state, app) = fixture().await;
    let recipe = json!({ "type": "library", "filters": {}, "sort": "name", "order": "asc" });
    let in_library = |library: &str| {
        let mut body = recipe_start(QUEUE, KEY, recipe.clone(), "900", 0);
        body["source"]["libraryId"] = json!(library);
        body
    };
    // Library 2 exists and user 2 does not reach it, and library 9 does not exist: the same
    // refusal, which does not depend on the track.
    let (status, unreachable) = start(&app, "user2", in_library("2")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unreachable}");
    let (_, missing) = start(&app, "user2", in_library("9")).await;
    assert_eq!(unreachable, missing);
    let mut other_track = in_library("2");
    other_track["startTrackId"] = json!("1");
    assert_eq!(start(&app, "user2", other_track).await.1, unreachable);
    let (status, _) = call(&app, "user2", "GET", "/api/v1/me/queue", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nothing started");

    // The owner reaches it, and user 2 the library they were granted.
    let (status, _) = start(&app, TOKEN, in_library("2")).await;
    assert_eq!(status, StatusCode::OK);
    let mut granted = recipe_start(QUEUE, KEY, recipe, "1", 0);
    granted["source"]["libraryId"] = json!("1");
    let (status, answer) = start(&app, "user2", granted).await;
    assert_eq!((status, &answer["length"]), (StatusCode::OK, &json!(14)));
}

#[tokio::test]
async fn recipes_the_server_cannot_resolve_are_refused() {
    let (_temp, _state, app) = fixture().await;
    let library = |sort: &str, order: &str, filters: Value| json!({ "type": "library", "filters": filters, "sort": sort, "order": order });
    let artist = |scope: &str, credit: &str| {
        json!({
            "type": "artist", "artistId": "10", "scope": scope, "artistCredit": credit, "sort": "name", "order": "asc",
        })
    };
    for (what, recipe) in [
        ("a personal sort", library("top", "asc", json!({}))),
        ("a sort with no direction", json!({ "type": "library", "filters": {}, "sort": "name" })),
        ("an unknown direction", library("name", "up", json!({}))),
        ("a sort and direction in one", library("name.asc", "asc", json!({}))),
        ("a filter", library("name", "asc", json!({ "genres": ["30"] }))),
        ("filters that are not a browse filter", library("name", "asc", json!([]))),
        ("a scope not built", artist("topTracks", "any")),
        ("an unknown credit", artist("all", "some")),
        ("an empty search", json!({ "type": "search", "q": "" })),
        ("a field it does not know", json!({ "type": "search", "q": "a", "limit": 5 })),
        ("explicit tracks", json!({ "type": "tracks" })),
    ] {
        let (status, body) = start(&app, TOKEN, recipe_start(QUEUE, KEY, recipe, "1", 0)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{what}: {body}");
    }
    // Tracks and a recipe together, a recipe with no start track, and tracks with one.
    let good = recipe_start(QUEUE, KEY, library("name", "asc", json!({})), "1", 0);
    let mut both = good.clone();
    both["source"]["tracks"] = json!(["1"]);
    let mut no_track = good.clone();
    no_track["startTrackId"] = Value::Null;
    let mut tracks_with_track = super::start_body(QUEUE, KEY, &["1"], 0);
    tracks_with_track["startTrackId"] = json!("1");
    for (what, body) in
        [("both", both), ("no start track", no_track), ("tracks", tracks_with_track)]
    {
        let (status, body) = start(&app, TOKEN, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{what}: {body}");
    }
    let (status, _) = call(&app, TOKEN, "GET", "/api/v1/me/queue", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nothing started");
}

#[tokio::test]
async fn an_oversized_recipe_is_refused_for_its_size() {
    let (_temp, _state, app) = fixture().await;
    // Read whole, a tagged recipe this size would be buffered whole before it is refused.
    let recipe = json!({ "type": "search", "q": "Song", "junk": vec![0; 100_000] });
    let (status, answer) = start(&app, TOKEN, recipe_start(QUEUE, KEY, recipe, "1", 0)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(answer["detail"], "A recipe is at most 4 KiB.");
}

#[tokio::test]
async fn a_desc_recipe_is_shown_as_sent() {
    let (_temp, _state, app) = fixture().await;
    let recipe = json!({
        "type": "artist", "artistId": "10", "scope": "all", "artistCredit": "owned",
        "sort": "releaseDate", "order": "desc",
    });
    let (status, answer) =
        start(&app, TOKEN, recipe_start(QUEUE, KEY, recipe.clone(), "1", 0)).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(snapshot(&app).await["state"]["sources"][0]["recipe"], recipe);
}

#[tokio::test]
async fn search_resolve_leaves_out_a_track_deleted_since_the_index() {
    let (_temp, state, _app) = fixture().await;
    let search = json!({ "type": "search", "q": "hymn" });
    assert_eq!(resolved(&state, &search).await.len(), 3);
    // A search after a change answers from the index already built and rebuilds only in the
    // background, so the resolve still reads the deleted track from it.
    state
        .db
        .write(|tx| tx.execute("DELETE FROM tracks WHERE id = 14", []).map(drop))
        .await
        .unwrap();
    let found = resolved(&state, &search).await;
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(!found.contains(&"14".to_owned()));
}
