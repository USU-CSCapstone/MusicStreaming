//! Recipes leave out what the caller hid, as the views they stand for do
//! (`requirements/conventions.md` §8), and only what that caller hid.

use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::playlist_recipes::{playlists, recipe as playlist};
use super::recipes::{fixture, recipe_start, resolved, resolved_as};
use super::{KEY, QUEUE, start};
use crate::api::scope::Caller;
use crate::api::testing::catalog::page_through;
use crate::api::testing::{TOKEN, call, json};

/// User 2, who reaches library 1 and hides nothing.
const USER2: Caller = Caller { user: 2, device: 3, admin: false };

/// What the owner hides: tracks of albums 101 and 102, credited to both artists, found by
/// searches for "song" and "aria", and every track of album 103.
const HIDDEN: [&str; 7] = ["1", "6", "13", "9", "10", "11", "14"];

/// Hides `tracks` of library 1 as the owner.
async fn hide(app: &Router, tracks: &[&str]) {
    for track in tracks {
        let uri = format!("/api/v1/libraries/1/tracks/{track}/hidden");
        assert_eq!(call(app, TOKEN, "PUT", &uri, None).await.0, StatusCode::NO_CONTENT);
    }
}

/// `ids` without the ones `HIDDEN` names, in the same order.
fn shown(ids: &[String]) -> Vec<String> {
    ids.iter().filter(|id| !HIDDEN.contains(&id.as_str())).cloned().collect()
}

/// The search section of tracks the owner sees for `q`, as track IDs.
async fn searched(app: &Router, q: &str) -> Vec<String> {
    let uri = format!("/api/v1/libraries/1/search?q={q}&types=tracks&sectionLimit=50");
    let (status, found) = json(app, &uri).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    found["sections"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|section| section["items"].as_array().unwrap())
        .map(|item| item["track"]["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn each_recipe_leaves_out_what_the_caller_hid() {
    let (_temp, state, app) = fixture().await;
    let views: Vec<(Value, String)> = [("{}", ""), (r#"{"tagAll":["30"]}"#, "&tagAll=30")]
        .into_iter()
        .flat_map(|(filters, tags)| {
            let filters: Value = serde_json::from_str(filters).unwrap();
            let with = move |mut recipe: Value| {
                recipe["filters"] = filters.clone();
                recipe
            };
            let albums = ["101", "102", "103"].map(|album| {
                (
                    with(json!({ "type": "album", "albumId": album, "sort": "name", "order": "asc" })),
                    format!("albumId={album}&sort=name&order=asc{tags}"),
                )
            });
            let artists = ["10", "11"].map(|artist| {
                (
                    with(json!({
                        "type": "artist", "artistId": artist, "scope": "all",
                        "artistCredit": "any", "sort": "duration", "order": "desc",
                    })),
                    format!("artistId={artist}&artistCredit=any&sort=duration&order=desc{tags}"),
                )
            });
            let library = (
                with(json!({ "type": "library", "sort": "dateAdded", "order": "asc" })),
                format!("sort=dateAdded&order=asc{tags}"),
            );
            albums.into_iter().chain(artists).chain([library])
        })
        .collect();
    let searches = ["song", "aria", "hymn"];
    let search = |q: &str| json!({ "type": "search", "q": q });

    let mut before = Vec::new();
    for (recipe, _) in &views {
        before.push(resolved(&state, recipe).await);
    }
    for q in searches {
        before.push(resolved(&state, &search(q)).await);
    }
    hide(&app, &HIDDEN).await;

    let mut lost = 0;
    for ((recipe, query), before) in views.iter().zip(&before) {
        let ids = resolved(&state, recipe).await;
        assert_eq!(ids, shown(before), "{recipe}");
        let paged =
            page_through(&app, &format!("/api/v1/libraries/1/tracks?{query}"), 2, "id").await;
        assert_eq!(json!(ids), paged, "{recipe} plays what its page shows");
        assert_eq!(&resolved_as(&state, USER2, recipe).await, before, "{recipe}, user 2's");
        lost += before.len() - ids.len();
    }
    for (q, before) in searches.into_iter().zip(&before[views.len()..]) {
        let ids = resolved(&state, &search(q)).await;
        assert_eq!(ids, shown(before), "{q}");
        assert_eq!(ids, searched(&app, q).await, "{q} plays what its search shows");
        assert_eq!(&resolved_as(&state, USER2, &search(q)).await, before, "{q}, user 2's");
        lost += before.len() - ids.len();
    }
    assert!(lost > 15, "the hidden tracks were in the lists");

    // An album whose every track is hidden resolves to nothing, as its page shows nothing.
    let album = json!({ "type": "album", "albumId": "103", "sort": "name", "order": "asc" });
    assert_eq!(resolved(&state, &album).await, Vec::<String>::new());

    // A start gets the list without them, under a hash of its own.
    let album = json!({ "type": "album", "albumId": "101", "sort": "name", "order": "asc" });
    let (status, started) =
        start(&app, TOKEN, recipe_start(QUEUE, KEY, album.clone(), "2", 0)).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let ids = resolved(&state, &album).await;
    assert_eq!(started["length"], json!(ids.len()));
    let (status, theirs) = start(&app, "user2", recipe_start(QUEUE, KEY, album, "2", 0)).await;
    assert_eq!(status, StatusCode::OK, "{theirs}");
    assert_eq!(theirs["length"], json!(ids.len() + 1), "user 2 has track 1");
    assert_ne!(started["list"], theirs["list"]);
}

#[tokio::test]
async fn a_playlist_recipe_leaves_out_what_its_owner_hid() {
    let (_temp, state, app) = fixture().await;
    playlists(&state).await;
    let recipes = [
        playlist(None, None, &json!({})),
        playlist(Some("name"), Some("desc"), &json!({})),
        playlist(Some("addedToPlaylist"), Some("asc"), &json!({ "tagAny": ["30", "31"] })),
    ];
    let mut before = Vec::new();
    for recipe in &recipes {
        before.push(resolved(&state, recipe).await);
    }
    // The missing track 9 among them, which a playlist holds in place.
    hide(&app, &HIDDEN).await;

    for (recipe, before) in recipes.iter().zip(&before) {
        let ids = resolved(&state, recipe).await;
        assert_eq!(ids, shown(before), "{recipe}");
        assert!(ids.len() < before.len(), "{recipe} held hidden tracks");
    }

    // The page holds every item, marking the hidden ones, and what is not marked is what plays.
    let uri = "/api/v1/libraries/1/playlists/501/items";
    let tracks = page_through(&app, uri, 4, "track").await;
    let marked = page_through(&app, uri, 4, "hidden").await;
    let unmarked: Vec<String> = tracks
        .as_array()
        .unwrap()
        .iter()
        .zip(marked.as_array().unwrap())
        .filter(|(_, hidden)| **hidden == json!(false))
        .map(|(track, _)| track["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(resolved(&state, &recipes[0]).await, unmarked);

    // User 2's own playlist holds track 1, which only the owner hid.
    let theirs = json!({ "type": "playlist", "playlistId": "502" });
    assert_eq!(resolved_as(&state, USER2, &theirs).await, ["1"]);
}
