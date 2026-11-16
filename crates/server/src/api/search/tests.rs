use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::testing::{catalog, eventually, json, send};
use super::*;
use crate::db::testing::{Album, Artist, Track, library};

/// [`catalog::app`], plus an empty library 3.
async fn app_with_catalog() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = catalog::app().await;
    db.write(|tx| library(tx, 3, "Empty", "/empty")).await.unwrap();
    (temp, db, app)
}

/// The item with `id` from a browse list, as the list shows it.
async fn listed(app: &Router, list: &str, id: &str) -> Value {
    let (_, page) = json(app, &format!("/api/v1/libraries/1/{list}?limit=1000")).await;
    page["items"].as_array().unwrap().iter().find(|item| item["id"] == id).unwrap().clone()
}

/// Each section's type and item IDs, in order.
fn sections(body: &Value) -> Vec<(String, Vec<String>)> {
    body["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|section| {
            let kind = section["type"].as_str().unwrap().to_owned();
            let key = kind.trim_end_matches('s').to_owned();
            let ids = section["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item[&key]["id"].as_str().unwrap().to_owned())
                .collect();
            (kind, ids)
        })
        .collect()
}

#[tokio::test]
async fn results_look_like_the_browse_lists() {
    let (_temp, _db, app) = app_with_catalog().await;
    let (status, body) = json(&app, "/api/v1/libraries/1/search?q=beatles").await;
    assert_eq!(status, StatusCode::OK);
    let artist = listed(&app, "artists", "10").await;
    assert_eq!(
        body,
        json!({
            "query": "beatles",
            "top": { "type": "artist", "artist": artist },
            "sections": [{ "type": "artists", "items": [{ "type": "artist", "artist": artist }], "total": 1 }],
            "nearMisses": [],
            "libraryEmpty": false,
        })
    );

    // A typo, and a track exactly as `listTracks` shows it.
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=come%20togther").await;
    assert_eq!(
        body["top"],
        json!({ "type": "track", "track": listed(&app, "tracks", "1001").await })
    );
    // Liked, it shows so here as there.
    let like = "/api/v1/libraries/1/tracks/1001/like";
    assert_eq!(send(app.clone(), "PUT", like).await.0, StatusCode::NO_CONTENT);
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=come%20togther").await;
    let track = listed(&app, "tracks", "1001").await;
    assert_eq!((&body["top"]["track"], &track["liked"]), (&track, &json!(true)));
}

#[tokio::test]
async fn sections_follow_the_core_and_the_filters() {
    let (_temp, _db, app) = app_with_catalog().await;
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=kid").await;
    assert_eq!(sections(&body), [("albums".to_owned(), vec!["102".to_owned()])]);
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=kid&types=tracks,playlists").await;
    assert_eq!(body["sections"], json!([]));
    assert_eq!(body["top"], Value::Null);
    // Equally exact everywhere: tracks, then albums, then artists.
    let (_, body) = json(&app, "/api/v1/libraries/2/search?q=elsewhere&sectionLimit=1").await;
    assert_eq!(
        sections(&body),
        [
            ("tracks".to_owned(), vec!["2001".to_owned()]),
            ("albums".to_owned(), vec!["201".to_owned()]),
            ("artists".to_owned(), vec!["20".to_owned()]),
        ]
    );
}

#[tokio::test]
async fn only_the_callers_library_is_searched() {
    let (_temp, _db, app) = app_with_catalog().await;
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=elsewhere").await;
    assert_eq!(body["sections"], json!([]));
    assert_eq!(body["libraryEmpty"], false);
    let (_, body) = json(&app, "/api/v1/libraries/3/search?q=elsewhere").await;
    assert_eq!(body["sections"], json!([]));
    assert_eq!(body["libraryEmpty"], true, "no music yet, not no match");
}

#[tokio::test]
async fn a_name_in_both_libraries_is_counted_once_in_each() {
    let (_temp, db, app) = app_with_catalog().await;
    // Library 1 gets a track, an album, and an artist named as library 2's are.
    db.write(|tx| {
        let title = "Elsewhere".to_owned();
        Track { title: title.clone(), ..Track::new(1007, 1, 101) }.insert(tx)?;
        Album { title: Some(title), ..Album::new(105, 1) }.insert(tx)?;
        Artist::new(14, 1, "Elsewhere").insert(tx)
    })
    .await
    .unwrap();
    let ids = |id: &str| vec![id.to_owned()];
    for (library, track, album, artist) in [(1, "1007", "105", "14"), (2, "2001", "201", "20")] {
        let uri = format!(
            "/api/v1/libraries/{library}/search?q=elsewhere&types=tracks,albums,artists&sectionLimit=1"
        );
        let (_, body) = json(&app, &uri).await;
        let expected = [
            ("tracks".to_owned(), ids(track)),
            ("albums".to_owned(), ids(album)),
            ("artists".to_owned(), ids(artist)),
        ];
        assert_eq!(sections(&body), expected, "library {library}");
        for section in body["sections"].as_array().unwrap() {
            assert_eq!(section["total"], 1, "library {library}: {section}");
        }
    }
}

#[tokio::test]
async fn what_is_gone_since_the_index_was_built_is_left_out() {
    let (_temp, db, app) = app_with_catalog().await;
    let uri = "/api/v1/libraries/1/search?q=untitled";
    let (_, body) = json(&app, uri).await;
    assert_eq!(sections(&body), [("tracks".to_owned(), vec!["1006".to_owned()])]);
    db.write(|tx| tx.execute_batch("DELETE FROM tracks WHERE id = 1006")).await.unwrap();
    // The search that notices the change answers from the index that still names the track.
    let (status, body) = json(&app, uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((&body["sections"], &body["top"]), (&json!([]), &Value::Null));
}

#[tokio::test]
async fn the_index_follows_the_library() {
    let (_temp, db, app) = app_with_catalog().await;
    let uri = "/api/v1/libraries/1/search?q=octopus";
    let (_, body) = json(&app, uri).await;
    assert_eq!(body["sections"], json!([]));
    db.write(|tx| {
        let title = "Octopus's Garden".into();
        Track { title, sort_key: b"o".to_vec(), ..Track::new(1007, 1, 101) }.insert(tx)
    })
    .await
    .unwrap();
    // The search that notices the change answers from the old index while a new one builds.
    let body = eventually("the index catches up", async || {
        let (_, body) = json(&app, uri).await;
        (body["sections"] != json!([])).then_some(body)
    })
    .await;
    assert_eq!(sections(&body), [("tracks".to_owned(), vec!["1007".to_owned()])]);
}

#[tokio::test]
async fn problems() {
    let (_temp, _db, app) = app_with_catalog().await;
    for (uri, status) in [
        ("/api/v1/libraries/9/search?q=a", StatusCode::NOT_FOUND),
        ("/api/v1/libraries/1/search", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=a&sectionLimit=0", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=a&sectionLimit=51", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=a&types=songs", StatusCode::UNPROCESSABLE_ENTITY),
        // Filters and sections not built yet are refused rather than ignored.
        ("/api/v1/libraries/1/search?q=a&types=lyrics", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=a&types=tracks,lyrics", StatusCode::UNPROCESSABLE_ENTITY),
        ("/api/v1/libraries/1/search?q=a&genre=rock", StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let (actual, content_type, _) = send(app.clone(), "GET", uri).await;
        assert_eq!(actual, status, "{uri}");
        assert_eq!(content_type.as_deref(), Some("application/problem+json"), "{uri}");
    }
}

/// A search with tags finds only what carries them, of every kind, and counts only that. Library
/// 1 gets three tracks called "Elsewhere": 1008, on the album "Elsewhere" (105) by the artist
/// "Elsewhere" (14), is Rock and Calm, which its album and artist take; 1009 is Calm; 1007 is
/// neither.
#[tokio::test]
async fn a_search_with_tags_finds_what_carries_them() {
    let (_temp, db, app) = app_with_catalog().await;
    db.write(|tx| {
        let title = || "Elsewhere".to_owned();
        Album { title: Some(title()), ..Album::new(105, 1) }.insert(tx)?;
        Artist::new(14, 1, "Elsewhere").insert(tx)?;
        for (id, album) in [(1007, 101), (1008, 105), (1009, 101)] {
            Track { title: title(), ..Track::new(id, 1, album) }.insert(tx)?;
        }
        tx.execute_batch(
            "INSERT INTO track_artists (library_id, track_id, position, artist_id, artist_name)
             VALUES (1, 1008, 0, 14, 'Elsewhere');
             INSERT INTO track_tags (library_id, track_id, tag_id, tag_name)
             VALUES (1, 1008, 30, 'Rock'), (1, 1008, 32, 'Calm'), (1, 1009, 32, 'Calm');",
        )
    })
    .await
    .unwrap();
    let ids = |ids: &[&str]| ids.iter().map(|&id| id.to_owned()).collect::<Vec<_>>();
    let all = [
        ("tracks".to_owned(), ids(&["1008"])),
        ("albums".to_owned(), ids(&["105"])),
        ("artists".to_owned(), ids(&["14"])),
    ];
    let base = "/api/v1/libraries/1/search?q=elsewhere";
    for query in ["tagAny=30", "tagAny=30,32", "tagAny=32&sectionLimit=1"] {
        let (status, body) = json(&app, &format!("{base}&{query}")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(sections(&body), all, "{query}");
    }
    let (_, body) = json(&app, &format!("{base}&tagAny=32")).await;
    assert_eq!(body["sections"][0]["total"], 2, "1008 and 1009");
    let (_, body) = json(&app, &format!("{base}&tagAny=30&types=tracks")).await;
    assert_eq!(sections(&body), all[..1]);
    for query in ["tagAny=31", "tagAll=30,31", "tagAny=nope"] {
        let (_, body) = json(&app, &format!("{base}&{query}")).await;
        assert_eq!(body["sections"], json!([]), "{query}");
    }
    let (_, body) = json(&app, "/api/v1/libraries/2/search?q=elsewhere&tagAny=30").await;
    assert_eq!(body["sections"], json!([]), "another library's tag");
}

/// Library 1 gets five tracks called "Lullaby" and a letter, 1101 to 1105, A to E.
async fn app_with_lullabies() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app_with_catalog().await;
    db.write(|tx| {
        for (id, letter) in (1101..).zip(["A", "B", "C", "D", "E"]) {
            Track { title: format!("Lullaby {letter}"), ..Track::new(id, 1, 101) }.insert(tx)?;
        }
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .unwrap();
    (temp, db, app)
}

/// Every page of the tracks section for `q`, read `limit` at a time from library 1, as each
/// page's track IDs.
async fn pages(app: &Router, q: &str, limit: u32) -> Vec<Vec<String>> {
    let mut pages = Vec::new();
    let mut cursor = String::new();
    loop {
        let uri = format!("/api/v1/libraries/1/search/tracks?q={q}&limit={limit}{cursor}");
        let (status, page) = json(app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        pages.push(track_ids(&page));
        match page["nextCursor"].as_str() {
            Some(next) => cursor = format!("&cursor={next}"),
            None => return pages,
        }
    }
}

/// The track IDs of a section page's items.
fn track_ids(page: &Value) -> Vec<String> {
    let items = page["items"].as_array().unwrap();
    items.iter().map(|item| item["track"]["id"].as_str().unwrap().to_owned()).collect()
}

#[tokio::test]
async fn a_section_pages_through_every_match_in_the_searchs_ranking() {
    let (_temp, _db, app) = app_with_lullabies().await;
    let (_, search) = json(&app, "/api/v1/libraries/1/search?q=lullaby&sectionLimit=50").await;
    assert_eq!(sections(&search)[0].1, ["1101", "1102", "1103", "1104", "1105"]);
    let pages = pages(&app, "lullaby", 2).await;
    assert_eq!(pages, [vec!["1101", "1102"], vec!["1103", "1104"], vec!["1105"]]);

    // Each page is the whole section's total, and its items are as `search` shows them.
    let (_, page) = json(&app, "/api/v1/libraries/1/search/tracks?q=lullaby&limit=1").await;
    assert_eq!(page["query"], "lullaby");
    assert_eq!(page["total"], 5);
    assert_eq!(page["items"][0], search["sections"][0]["items"][0]);
    let (_, page) = json(&app, "/api/v1/libraries/1/search/tracks?q=lullaby").await;
    assert_eq!((track_ids(&page).len(), &page["nextCursor"]), (5, &Value::Null), "100 by default");
}

#[tokio::test]
async fn a_page_continues_after_the_last_match_once_the_index_is_rebuilt() {
    let (_temp, db, app) = app_with_lullabies().await;
    let (_, first) = json(&app, "/api/v1/libraries/1/search/tracks?q=lullaby&limit=2").await;
    assert_eq!(track_ids(&first), ["1101", "1102"]);
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    // One that ranks before the cursor is added, and one after it removed.
    db.write(|tx| {
        Track { title: "Lullaby AA".into(), ..Track::new(1106, 1, 101) }.insert(tx)?;
        tx.execute_batch("DELETE FROM tracks WHERE id = 1104")
    })
    .await
    .unwrap();
    // Once the index is rebuilt, which finding the new track shows.
    eventually("the index catches up", async || {
        let (_, page) = json(&app, "/api/v1/libraries/1/search/tracks?q=lullaby%20aa").await;
        (track_ids(&page) == ["1106"]).then_some(())
    })
    .await;
    let uri = format!("/api/v1/libraries/1/search/tracks?q=lullaby&limit=10&cursor={cursor}");
    let (_, page) = json(&app, &uri).await;
    assert_eq!(page["total"], 5);
    assert_eq!(
        track_ids(&page),
        ["1103", "1105"],
        "nothing twice, nothing still there passed over"
    );
}

#[tokio::test]
async fn a_section_leaves_out_what_the_caller_hid_and_what_lacks_the_tags() {
    let (_temp, db, app) = app_with_lullabies().await;
    let hide = "/api/v1/libraries/1/tracks/1102/hidden";
    assert_eq!(send(app.clone(), "PUT", hide).await.0, StatusCode::NO_CONTENT);
    let (_, page) = json(&app, "/api/v1/libraries/1/search/tracks?q=lullaby").await;
    assert_eq!(
        (track_ids(&page), &page["total"]),
        (vec!["1101", "1103", "1104", "1105"].into_iter().map(String::from).collect(), &json!(4))
    );

    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO track_tags (library_id, track_id, tag_id, tag_name)
             VALUES (1, 1103, 30, 'Rock'), (1, 1105, 30, 'Rock');",
        )
    })
    .await
    .unwrap();
    let (_, page) = json(&app, "/api/v1/libraries/1/search/tracks?q=lullaby&tagAny=30").await;
    assert_eq!(
        (track_ids(&page), &page["total"]),
        (vec!["1103".to_owned(), "1105".to_owned()], &json!(2))
    );
}

#[tokio::test]
async fn section_problems() {
    let (_temp, _db, app) = app_with_lullabies().await;
    let (_, page) = json(&app, "/api/v1/libraries/1/search/albums?q=lullaby&limit=1").await;
    let albums = page["nextCursor"].as_str().map(str::to_owned);
    assert_eq!(albums, None, "no album matches");
    let (_, page) = json(&app, "/api/v1/libraries/1/search/tracks?q=lullaby&limit=1").await;
    let tracks = page["nextCursor"].as_str().unwrap().to_owned();
    let base = "/api/v1/libraries/1/search";
    for (uri, status) in [
        ("/api/v1/libraries/9/search/tracks?q=a".to_owned(), StatusCode::NOT_FOUND),
        (format!("{base}/songs?q=a"), StatusCode::NOT_FOUND),
        (format!("{base}/lyrics?q=a"), StatusCode::UNPROCESSABLE_ENTITY),
        (format!("{base}/tracks"), StatusCode::UNPROCESSABLE_ENTITY),
        (format!("{base}/tracks?q="), StatusCode::UNPROCESSABLE_ENTITY),
        (format!("{base}/tracks?q=a&limit=0"), StatusCode::UNPROCESSABLE_ENTITY),
        (format!("{base}/tracks?q=a&limit=1001"), StatusCode::UNPROCESSABLE_ENTITY),
        (format!("{base}/tracks?q=a&cursor=nope"), StatusCode::UNPROCESSABLE_ENTITY),
        (format!("{base}/albums?q=a&cursor={tracks}"), StatusCode::UNPROCESSABLE_ENTITY),
        (format!("{base}/tracks?q=a&sectionLimit=5"), StatusCode::UNPROCESSABLE_ENTITY),
    ] {
        let (actual, content_type, _) = send(app.clone(), "GET", &uri).await;
        assert_eq!(actual, status, "{uri}");
        assert_eq!(content_type.as_deref(), Some("application/problem+json"), "{uri}");
    }
    // A list cursor's sort values are not a rank.
    let list = crate::api::cursor::encode("search.tracks", &[rusqlite::types::Value::Integer(1)]);
    let (status, _, _) =
        send(app.clone(), "GET", &format!("{base}/tracks?q=a&cursor={list}")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, page) = json(&app, &format!("{base}/playlists?q=lullaby")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page, json!({ "query": "lullaby", "items": [], "nextCursor": null, "total": 0 }));
}
