//! Queues started from a view's list query, which the server runs to the end.

use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{KEY, get, post};
use crate::api::testing::{TOKEN, add_device, add_user, catalog, conn, json};
use crate::db::testing::{Track, access};

/// [`catalog::app`], with user 2, who reaches only library 2, on device 2 as `user2`.
async fn setup() -> (tempfile::TempDir, Router) {
    let (temp, _db, app) = catalog::app().await;
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user2");
    access(&conn, 2, 2).unwrap();
    (temp, app)
}

/// A request to start queue number `n` over `query` of `library`, at `track`.
fn view(n: usize, library: &str, query: Value, track: &str) -> Value {
    json!({
        "queueId": format!("5f0c6a52-3b7e-4d43-9a8e-0d1c2b3a{n:04x}"),
        "context": { "type": "library" }, "startTrackId": track,
        "source": { "key": KEY, "libraryId": library, "query": query },
    })
}

/// The IDs of the tracks `uri` lists, paged to the end.
async fn paged(app: &Router, uri: &str) -> Vec<String> {
    let ids = catalog::page_through(app, uri, 2, "id").await;
    ids.as_array().unwrap().iter().map(|id| id.as_str().unwrap().to_owned()).collect()
}

/// Starts each of `cases`, a query and the paged list it must match, at the `at`th track of the
/// list, and checks the queue's one source is that list and the queue is at that track.
async fn assert_start_matches_paging(app: &Router, cases: &[(Value, &str)], at: usize) {
    for (n, (query, uri)) in cases.iter().enumerate() {
        let expected = paged(app, uri).await;
        assert!(expected.len() > at.max(1), "{uri} is too short to tell its order");
        let body = view(n, "1", query.clone(), &expected[at]);
        assert_eq!(post(app, TOKEN, body).await, StatusCode::NO_CONTENT, "{query}");
        let state = get(app, TOKEN).await.1["state"].clone();
        assert_eq!(state["sources"][0]["tracks"], json!(expected), "{query}");
        assert_eq!(state["current"], json!(at), "{query}");
    }
}

#[tokio::test]
async fn a_library_start_has_the_order_of_the_paged_list() {
    let (_temp, app) = setup().await;
    let tracks = "/api/v1/libraries/1/tracks";
    let cases = [
        (json!({ "type": "tracks" }), tracks.to_owned()),
        (
            json!({ "type": "tracks", "sort": "artist", "order": "desc" }),
            format!("{tracks}?sort=artist&order=desc"),
        ),
        (json!({ "type": "tracks", "sort": "releaseDate" }), format!("{tracks}?sort=releaseDate")),
        (
            json!({ "type": "tracks", "sort": "dateAdded", "order": "desc" }),
            format!("{tracks}?sort=dateAdded&order=desc"),
        ),
    ];
    let cases: Vec<_> = cases.iter().map(|(query, uri)| (query.clone(), uri.as_str())).collect();
    assert_start_matches_paging(&app, &cases, 0).await;
}

#[tokio::test]
async fn an_artist_start_has_the_order_of_the_paged_list() {
    let (_temp, app) = setup().await;
    let tracks = "/api/v1/libraries/1/tracks";
    // Radiohead: a track on their album, and one on The Beatles' where they are credited.
    let cases = [
        (
            json!({ "type": "tracks", "artistId": "11", "artistCredit": "any", "sort": "album" }),
            format!("{tracks}?artistId=11&artistCredit=any&sort=album"),
        ),
        (json!({ "type": "tracks", "artistId": "10" }), format!("{tracks}?artistId=10")),
    ];
    let cases: Vec<_> = cases.iter().map(|(query, uri)| (query.clone(), uri.as_str())).collect();
    assert_start_matches_paging(&app, &cases, 1).await;
}

#[tokio::test]
async fn a_search_start_has_the_order_of_the_searched_tracks() {
    let (_temp, db, app) = catalog::app().await;
    db.write(|tx| {
        for (id, title) in [(1101, "Together Again"), (1102, "Altogether Now"), (1103, "Together")]
        {
            let sort_key = title.to_lowercase().into_bytes();
            Track { title: title.into(), sort_key, ..Track::new(id, 1, 101) }.insert(tx)?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let (_, found) =
        json(&app, "/api/v1/libraries/1/search?q=together&types=tracks&sectionLimit=50").await;
    let expected: Vec<&str> = found["sections"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["track"]["id"].as_str().unwrap())
        .collect();
    assert!(expected.len() > 1, "{found}");
    let body = view(0, "1", json!({ "type": "search", "q": "together" }), expected[1]);
    assert_eq!(post(&app, TOKEN, body).await, StatusCode::NO_CONTENT);
    let state = get(&app, TOKEN).await.1["state"].clone();
    assert_eq!(state["sources"][0]["tracks"], json!(expected));
    assert_eq!(state["current"], json!(1));
}

#[tokio::test]
async fn a_start_track_that_is_not_first_starts_there_and_one_that_is_absent_is_refused() {
    let (_temp, app) = setup().await;
    let expected = paged(&app, "/api/v1/libraries/1/tracks").await;
    let last = expected.len() - 1;
    assert_start_matches_paging(
        &app,
        &[(json!({ "type": "tracks" }), "/api/v1/libraries/1/tracks")],
        last,
    )
    .await;

    // Library 2's track is not in library 1's list.
    let body = view(9, "1", json!({ "type": "tracks" }), "3001");
    assert_eq!(post(&app, TOKEN, body).await, StatusCode::UNPROCESSABLE_ENTITY);
    let body = view(9, "1", json!({ "type": "tracks" }), "not an id");
    assert_eq!(post(&app, TOKEN, body).await, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn a_library_the_caller_does_not_reach_is_refused_as_an_unknown_one_is() {
    let (_temp, app) = setup().await;
    let first = paged(&app, "/api/v1/libraries/1/tracks").await[0].clone();
    for (library, query) in [
        ("1", json!({ "type": "tracks" })),
        ("1", json!({ "type": "search", "q": "t" })),
        ("999", json!({ "type": "tracks" })),
    ] {
        let body = view(0, library, query.clone(), &first);
        assert_eq!(post(&app, "user2", body).await, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
    }
    assert_eq!(get(&app, "user2").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_query_the_list_would_refuse_or_a_source_of_both_is_refused() {
    let (_temp, app) = setup().await;
    let first = paged(&app, "/api/v1/libraries/1/tracks").await[0].clone();
    let bad = [
        json!({ "type": "tracks", "sort": "nonsense" }),
        json!({ "type": "tracks", "sort": "playCount" }),
        json!({ "type": "tracks", "unknown": 1 }),
        json!({ "type": "tracks", "cursor": "abc" }),
        json!({ "type": "tracks", "limit": 5 }),
        json!({ "type": "tracks", "ids": "1,2" }),
        json!({ "type": "search", "q": "" }),
        json!({ "type": "search", "q": "t", "sectionLimit": 5 }),
        json!({ "type": "playlist" }),
        json!({}),
    ];
    for query in bad {
        let body = view(0, "1", query.clone(), &first);
        assert_eq!(post(&app, TOKEN, body).await, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
    }
    // Tracks and a query, a query with an index, and tracks with a track to start at.
    let mut both = view(0, "1", json!({ "type": "tracks" }), &first);
    both["source"]["tracks"] = json!([first]);
    let mut indexed = view(0, "1", json!({ "type": "tracks" }), &first);
    indexed["start"] = json!(0);
    let mut named = super::start("5f0c6a52-3b7e-4d43-9a8e-0d1c2b3a4f5e", "1", &[&first]);
    named["startTrackId"] = json!(first);
    for body in [both, indexed, named] {
        assert_eq!(post(&app, TOKEN, body).await, StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(get(&app, TOKEN).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_list_of_more_than_one_internal_page_and_a_long_search_start_whole() {
    let (_temp, db, app) = catalog::app().await;
    // 1,100 tracks, past the 1,000 the server reads at a time, a third with no release date,
    // and some with an empty artist key, so the unknown parts of the sorts cross a page.
    db.write(|tx| {
        for id in 10_000..11_100_i64 {
            let title = format!("Together {id}");
            let track = Track {
                title: title.clone(),
                sort_key: title.to_lowercase().into_bytes(),
                ..Track::new(id, 1, 101)
            };
            let release_date = (id % 3 != 0).then(|| format!("19{:02}", id % 100));
            let artist_sort_key =
                if id % 5 == 0 { Vec::new() } else { format!("artist {}", id % 7).into_bytes() };
            Track { release_date, artist_sort_key, ..track }.insert(tx)?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let tracks = "/api/v1/libraries/1/tracks";
    let cases = [
        (json!({ "type": "tracks" }), tracks.to_owned()),
        (
            json!({ "type": "tracks", "sort": "releaseDate", "order": "desc" }),
            format!("{tracks}?sort=releaseDate&order=desc"),
        ),
        (
            json!({ "type": "tracks", "sort": "artist", "order": "desc" }),
            format!("{tracks}?sort=artist&order=desc"),
        ),
    ];
    let cases: Vec<_> = cases.iter().map(|(query, uri)| (query.clone(), uri.as_str())).collect();
    assert_start_matches_paging(&app, &cases, 1050).await;

    let (_, found) =
        json(&app, "/api/v1/libraries/1/search?q=together&types=tracks&sectionLimit=50").await;
    let shown: Vec<&str> = found["sections"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["track"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(shown.len(), 50);
    let body = view(7, "1", json!({ "type": "search", "q": "together" }), shown[0]);
    assert_eq!(post(&app, TOKEN, body).await, StatusCode::NO_CONTENT);
    let state = get(&app, TOKEN).await.1["state"].clone();
    let queued = state["sources"][0]["tracks"].as_array().unwrap();
    assert!(queued.len() > 1100, "{}", queued.len());
    assert_eq!(&queued[..50], shown);
}
