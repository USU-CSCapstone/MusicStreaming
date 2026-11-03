//! Playlist recipes: resolving the caller's own playlist as its page shows it.

use axum::http::StatusCode;
use jewelcase_core::{SortRow, sort_rows};
use serde_json::{Value, json};

use super::recipes::{fixture, recipe_start, resolved};
use super::{KEY, QUEUE, snapshot, start};
use crate::api::AppState;
use crate::api::testing::catalog::page_through;
use crate::api::testing::{TOKEN, call, json};

/// Playlist 501 (the owner's, library 1, version 7) holds tracks of the fixture's library with
/// duplicates, items added at the same time and two missing tracks (7 and 9), as (track, added at).
/// Playlist 502 is user 2's in library 1, 503 the owner's in library 2, and 504 user 2's in
/// library 2, which they do not reach.
const ITEMS: [(i64, i64); 19] = [
    (5, 3000),
    (1, 1000),
    (5, 1000),
    (9, 2000),
    (2, 1000),
    (14, 5000),
    (1, 1000),
    (7, 2000),
    (12, 4000),
    (3, 4000),
    (3, 4000),
    (11, 2000),
    (6, 3000),
    (13, 5000),
    (10, 1000),
    (8, 2000),
    (4, 6000),
    (6, 3000),
    (2, 7000),
];

/// Adds the playlists to the fixture, as the owner's and user 2's.
async fn playlists(state: &AppState) {
    state
        .db
        .write(|tx| {
            for (id, library, user, version) in
                [(501, 1, 1, 7), (502, 1, 2, 1), (503, 2, 1, 1), (504, 2, 2, 1)]
            {
                tx.execute(
                    "INSERT INTO playlists (id, library_id, user_id, title, folded, sort_key, \
                     version, created_at, modified_at) VALUES (?1, ?2, ?3, 'P', 'p', x'70', ?4, 0, 0)",
                    rusqlite::params![id, library, user, version],
                )?;
            }
            for (at, (track, added)) in (1_i64..).zip(ITEMS) {
                tx.execute(
                    "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
                     VALUES (501, ?1, 1, ?2, ?3)",
                    rusqlite::params![at << 20, track, added],
                )?;
            }
            tx.execute(
                "INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at) \
                 VALUES (502, 1, 1, 1, 1), (503, 1, 2, 900, 1), (504, 1, 2, 900, 1)",
                [],
            )?;
            tx.execute("UPDATE tracks SET missing_since = 1 WHERE id IN (7, 9)", [])?;
            tx.execute("UPDATE playlists SET track_count = 19 WHERE id = 501", [])?;
            Ok(())
        })
        .await
        .unwrap();
}

/// The playlist recipe for 501 with a sort, direction and filters, each left out when `None`.
fn recipe(sort: Option<&str>, order: Option<&str>, filters: &Value) -> Value {
    let mut recipe = json!({ "type": "playlist", "playlistId": "501" });
    for (name, value) in [("sort", sort), ("order", order)] {
        if let Some(value) = value {
            recipe[name] = json!(value);
        }
    }
    if filters.as_object().is_some_and(|filters| !filters.is_empty()) {
        recipe["filters"] = filters.clone();
    }
    recipe
}

/// The tag type of each tag of the fixture.
fn tag_type(tag: &str) -> &'static str {
    if tag == "32" { "mood" } else { "genre" }
}

/// Whether a track's `tags` pass `filters` as the page filters: every `tagAll` one, and of each
/// type `tagAny` names, one of them.
fn passes(tags: &Value, filters: &Value) -> bool {
    let held: Vec<&str> = tags
        .as_object()
        .unwrap()
        .values()
        .flat_map(|tags| tags.as_array().unwrap())
        .map(|tag| tag["id"].as_str().unwrap())
        .collect();
    let named = |list: &str| -> Vec<&str> {
        filters[list].as_array().into_iter().flatten().map(|tag| tag.as_str().unwrap()).collect()
    };
    let (all, any) = (named("tagAll"), named("tagAny"));
    let types: std::collections::BTreeSet<_> = any.iter().map(|tag| tag_type(tag)).collect();
    all.iter().all(|tag| held.contains(tag))
        && types.iter().all(|t| any.iter().any(|tag| tag_type(tag) == *t && held.contains(tag)))
}

/// The track IDs the playlist's page shows with `sort` and `filters`: the items endpoint's rows,
/// sorted by the core's comparator on the keys the web builds, then filtered.
async fn page(app: &axum::Router, sort: Option<(&str, bool)>, filters: &Value) -> Vec<String> {
    let uri = "/api/v1/libraries/1/playlists/501/items";
    let tracks = page_through(app, uri, 4, "track").await;
    let added = page_through(app, uri, 4, "addedAt").await;
    let rows: Vec<(Value, Value)> = tracks
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .zip(added.as_array().unwrap().iter().cloned())
        .collect();
    let order: Vec<usize> = match sort {
        None => (0..rows.len()).collect(),
        Some((field, descending)) => {
            let keys: Vec<SortRow> = rows
                .iter()
                .map(|(t, added)| {
                    serde_json::from_value(json!({
                        "id": t["id"], "titleKey": t["sortKeys"]["title"],
                        "artistKey": t["sortKeys"]["artist"], "albumKey": t["sortKeys"]["album"],
                        "albumId": t["album"]["id"], "releaseDate": t["releaseDate"],
                        "disc": t["discNumber"], "number": t["trackNumber"],
                        "durationUs": t["durationUs"], "added": added,
                    }))
                    .unwrap()
                })
                .collect();
            sort_rows(&keys, serde_json::from_value(json!(field)).unwrap(), descending)
        }
    };
    order
        .into_iter()
        .filter(|&at| passes(&rows[at].0["tags"], filters))
        .map(|at| rows[at].0["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn a_playlist_recipe_resolves_the_rows_its_page_shows() {
    let (_temp, state, app) = fixture().await;
    playlists(&state).await;
    let filters = [
        json!({}),
        json!({ "tagAll": ["30"] }),
        json!({ "tagAny": ["30", "31"] }),
        json!({ "tagAll": ["32"], "tagAny": ["30", "31"] }),
        json!({ "tagAll": ["99"] }),
    ];

    // Its own order keeps duplicates and the missing tracks 7 and 9, in place.
    let own = resolved(&state, &recipe(None, None, &json!({}))).await;
    let stored: Vec<String> = ITEMS.iter().map(|(track, _)| track.to_string()).collect();
    assert_eq!(own, stored);

    let mut differs = 0;
    for filters in &filters {
        assert_eq!(
            resolved(&state, &recipe(None, None, filters)).await,
            page(&app, None, filters).await
        );
        for field in ["name", "artist", "album", "releaseDate", "addedToPlaylist", "duration"] {
            for order in ["asc", "desc"] {
                let shown = page(&app, Some((field, order == "desc")), filters).await;
                let recipe = recipe(Some(field), Some(order), filters);
                assert_eq!(resolved(&state, &recipe).await, shown, "{recipe}");
                differs += usize::from(shown != page(&app, None, filters).await);
            }
        }
    }
    assert!(differs > 20, "the sorts are not all the playlist's own order");

    let up = resolved(&state, &recipe(Some("addedToPlaylist"), Some("asc"), &json!({}))).await;
    // Items added at the same time, and a track twice, end on the ID and then the item's place.
    assert_eq!(&up[..5], ["1", "1", "2", "5", "10"].map(String::from));
}

#[tokio::test]
async fn a_started_playlist_records_the_version_it_resolved() {
    let (_temp, state, app) = fixture().await;
    playlists(&state).await;
    let sorted = recipe(Some("releaseDate"), Some("desc"), &json!({ "tagAny": ["30", "31"] }));
    let body = recipe_start(QUEUE, KEY, sorted.clone(), "6", 0);
    let (status, answer) = start(&app, TOKEN, body).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let mut expected = sorted.clone();
    expected["version"] = json!(7);
    assert_eq!(snapshot(&app).await["state"]["sources"][0]["recipe"], expected);
    assert_eq!(answer["length"], resolved(&state, &sorted).await.len());

    // The own order leaves its sort out, and a later resolve reads the version then.
    state
        .db
        .write(|tx| tx.execute("UPDATE playlists SET version = 8 WHERE id = 501", []))
        .await
        .unwrap();
    let caller = crate::api::scope::Caller { user: 1, device: 1, admin: true };
    let plain: crate::api::queue::Recipe =
        serde_json::from_value(recipe(None, None, &json!({}))).unwrap();
    let (_, stored) =
        crate::api::queue::resolve(&state.db, &state.indexes, &caller, crate::api::Id(1), &plain)
            .await
            .unwrap();
    assert_eq!(
        serde_json::to_value(stored).unwrap(),
        json!({ "type": "playlist", "playlistId": "501", "version": 8 })
    );
    let up: crate::api::queue::Recipe =
        serde_json::from_value(recipe(Some("name"), Some("asc"), &json!({}))).unwrap();
    let (_, stored) =
        crate::api::queue::resolve(&state.db, &state.indexes, &caller, crate::api::Id(1), &up)
            .await
            .unwrap();
    assert_eq!(
        serde_json::to_value(stored).unwrap(),
        json!({ "type": "playlist", "playlistId": "501", "version": 8, "sort": "name", "order": "asc" })
    );
}

#[tokio::test]
async fn only_the_callers_own_playlist_in_a_library_they_reach_resolves() {
    let (_temp, state, app) = fixture().await;
    playlists(&state).await;
    let playlist = |id: &str, library: &str| {
        let mut body = recipe_start(
            QUEUE,
            KEY,
            json!({ "type": "playlist", "playlistId": id, "sort": "name", "order": "asc" }),
            "1",
            0,
        );
        body["source"]["libraryId"] = json!(library);
        body
    };
    let (status, found) = start(&app, "user2", playlist("501", "1")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{found}");
    let (status, api) = call(&app, "user2", "GET", "/api/v1/libraries/1/playlists/501", None).await;
    assert_eq!((status, &api["code"]), (StatusCode::NOT_FOUND, &found["code"]));

    // Another account's, an admin's included, one that is in another library, one that never
    // was, and an ID that is not one, are all absent alike.
    for (what, token, id, library) in [
        ("an admin and another's", TOKEN, "502", "1"),
        ("another library's", TOKEN, "503", "1"),
        ("one the library lacks", TOKEN, "999", "1"),
        ("not an ID", TOKEN, "x", "1"),
        ("another's in the other library", TOKEN, "504", "2"),
    ] {
        let (status, body) = start(&app, token, playlist(id, library)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{what}: {body}");
        assert_eq!(body["code"], found["code"], "{what}");
    }

    // A library the caller cannot reach is refused as any recipe is, with their own playlist
    // in it or none, and library 9 which does not exist.
    let (status, unreachable) = start(&app, "user2", playlist("504", "2")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{unreachable}");
    assert_eq!(start(&app, "user2", playlist("999", "9")).await.1, unreachable);
    let (status, _) = call(&app, "user2", "GET", "/api/v1/me/queue", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nothing started");
    let (status, _) = json(&app, "/api/v1/me/queue").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nothing started");

    // The owner's own resolves, as many tracks as it holds.
    let (status, started) = start(&app, TOKEN, playlist("501", "1")).await;
    assert_eq!((status, &started["length"]), (StatusCode::OK, &json!(ITEMS.len())));
}

#[tokio::test]
async fn playlist_recipes_the_server_cannot_resolve_are_refused() {
    let (_temp, state, app) = fixture().await;
    playlists(&state).await;
    for (what, recipe) in [
        (
            "an order without a sort",
            json!({ "type": "playlist", "playlistId": "501", "order": "desc" }),
        ),
        ("a sort the page lacks", recipe(Some("dateAdded"), Some("asc"), &json!({}))),
        ("the page's own name for a sort", recipe(Some("title"), Some("asc"), &json!({}))),
        ("an unknown direction", recipe(Some("name"), Some("up"), &json!({}))),
        ("a version", json!({ "type": "playlist", "playlistId": "501", "version": 7 })),
        ("a filter", recipe(None, None, &json!({ "genres": ["30"] }))),
        ("no playlist", json!({ "type": "playlist" })),
    ] {
        let (status, body) = start(&app, TOKEN, recipe_start(QUEUE, KEY, recipe, "1", 0)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{what}: {body}");
    }
    let (status, _) = call(&app, TOKEN, "GET", "/api/v1/me/queue", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "nothing started");
}

#[tokio::test]
async fn a_start_at_a_tracks_second_place_plays_there() {
    let (_temp, state, app) = fixture().await;
    playlists(&state).await;
    // Track 5 is items 0 and 2 of the playlist's own order.
    let body = recipe_start(QUEUE, KEY, recipe(None, None, &json!({})), "5", 2);
    let (status, answer) = start(&app, TOKEN, body).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(snapshot(&app).await["state"]["current"], json!(format!("{KEY}.2")));
}
