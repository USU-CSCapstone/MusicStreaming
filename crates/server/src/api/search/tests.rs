use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::super::testing::{
    TOKEN, add_device, add_user, call, catalog, conn, eventually, json, send,
};
use super::*;
use crate::db::testing::{Album, Artist, Track, access, library};

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

/// Track `id` of library 1 on album 101 called `title`, sorting by it as the scanner sorts it.
fn titled(id: i64, title: &str) -> Track {
    let sort_key = jewelcase_core::sort_key(title, None).into_bytes();
    Track { title: title.to_owned(), sort_key, ..Track::new(id, 1, 101) }
}

/// Library 1 gets five tracks called "Lullaby" and a letter, 1101 to 1105, A to E.
async fn app_with_lullabies() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app_with_catalog().await;
    db.write(|tx| {
        for (id, letter) in (1101..).zip(["A", "B", "C", "D", "E"]) {
            titled(id, &format!("Lullaby {letter}")).insert(tx)?;
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
    section_pages(app, "tracks", &format!("q={q}&limit={limit}")).await
}

/// Every page of library 1's `section` for `query`, as each page's item IDs.
async fn section_pages(app: &Router, section: &str, query: &str) -> Vec<Vec<String>> {
    let mut pages = Vec::new();
    let mut cursor = String::new();
    loop {
        let uri = format!("/api/v1/libraries/1/search/{section}?{query}{cursor}");
        let (status, page) = json(app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        pages.push(item_ids(&page));
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

/// The IDs of a section page's items, of whatever type.
fn item_ids(page: &Value) -> Vec<String> {
    let items = page["items"].as_array().unwrap();
    let id = |item: &Value| item[item["type"].as_str().unwrap()]["id"].as_str().unwrap().to_owned();
    items.iter().map(id).collect()
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
        titled(1106, "Lullaby AA").insert(tx)?;
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

#[tokio::test]
async fn a_section_sorts_its_matches_as_its_list_does() {
    let (_temp, db, app) = app_with_lullabies().await;
    // Longest first, and the last added shortest; two albums, and a track that does not match.
    db.write(|tx| {
        for (id, seconds) in (1101..).zip([3, 5, 1, 4, 2]) {
            tx.execute(
                "UPDATE tracks SET duration_us = ?2 WHERE id = ?1",
                (id, seconds * 1_000_000),
            )?;
        }
        for (id, title) in [(106, "Lullaby Two"), (107, "Lullaby One")] {
            let sort_key = jewelcase_core::sort_key(title, None).into_bytes();
            Album { title: Some(title.into()), sort_key, ..Album::new(id, 1) }.insert(tx)?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let ids = |ids: &[&str]| ids.iter().map(|&id| id.to_owned()).collect::<Vec<_>>();
    let pages = section_pages(&app, "tracks", "q=lullaby&sort=name&order=desc&limit=2").await;
    assert_eq!(pages, [ids(&["1105", "1104"]), ids(&["1103", "1102"]), ids(&["1101"])]);
    let pages = section_pages(&app, "tracks", "q=lullaby&sort=duration&limit=3").await;
    assert_eq!(pages, [ids(&["1103", "1105", "1101"]), ids(&["1104", "1102"])]);
    let pages = section_pages(&app, "albums", "q=lullaby&sort=name").await;
    assert_eq!(pages, [ids(&["107", "106"])]);

    // The total is the section's, and the items are as the ranked section shows them.
    let base = "/api/v1/libraries/1/search/tracks?q=lullaby";
    let (_, sorted) = json(&app, &format!("{base}&sort=name&limit=1")).await;
    let (_, ranked) = json(&app, &format!("{base}&limit=1")).await;
    assert_eq!(sorted["total"], 5);
    assert_eq!(sorted["items"], ranked["items"], "A is first both ways");

    // What the caller hid is left out of a sorted section, and out of its total.
    let hide = "/api/v1/libraries/1/tracks/1103/hidden";
    assert_eq!(send(app.clone(), "PUT", hide).await.0, StatusCode::NO_CONTENT);
    let (_, page) = json(&app, &format!("{base}&sort=duration")).await;
    assert_eq!(
        (item_ids(&page), &page["total"]),
        (ids(&["1105", "1101", "1104", "1102"]), &json!(4))
    );
}

#[tokio::test]
async fn a_sorted_section_takes_only_its_lists_sorts_and_cursors() {
    let (_temp, _db, app) = app_with_lullabies().await;
    let base = "/api/v1/libraries/1/search";
    let cursor = |uri: String| {
        let app = app.clone();
        async move { json(&app, &uri).await.1["nextCursor"].as_str().unwrap().to_owned() }
    };
    let ranked = cursor(format!("{base}/tracks?q=lullaby&limit=1")).await;
    let sorted = cursor(format!("{base}/tracks?q=lullaby&sort=name&limit=1")).await;
    for uri in [
        format!("{base}/tracks?q=a&sort=nope"),
        format!("{base}/tracks?q=a&sort=playCount"),
        format!("{base}/albums?q=a&sort=album"),
        format!("{base}/artists?q=a&sort=releaseDate"),
        format!("{base}/tracks?q=a&order=desc"),
        format!("{base}/tracks?q=a&sort=name&cursor={ranked}"),
        format!("{base}/tracks?q=a&sort=name&order=desc&cursor={sorted}"),
        format!("{base}/tracks?q=a&cursor={sorted}"),
    ] {
        let (status, _, _) = send(app.clone(), "GET", &uri).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{uri}");
    }
}

#[tokio::test]
async fn a_sorted_section_reads_the_same_walked_or_led() {
    let (_temp, db, app) = app_with_lullabies().await;
    // "Lullaby F" has no sortable title, so it sorts last either way. Fillers sort ahead of every
    // lullaby by name, so a walk's first read is all fillers and it reads again.
    let fill = |ids: std::ops::Range<i64>| {
        let db = db.clone();
        async move {
            db.write(move |tx| {
                for id in ids {
                    titled(id, &format!("Filler {id}")).insert(tx)?;
                }
                Ok::<_, rusqlite::Error>(())
            })
            .await
            .unwrap();
        }
    };
    db.write(|tx| Track { sort_key: Vec::new(), ..titled(1106, "Lullaby F") }.insert(tx))
        .await
        .unwrap();
    fill(5000..5300).await;
    let ids = |ids: [&str; 2]| ids.map(str::to_owned).to_vec();
    let asc = [ids(["1101", "1102"]), ids(["1103", "1104"]), ids(["1105", "1106"])];
    let desc = [ids(["1105", "1104"]), ids(["1103", "1102"]), ids(["1101", "1106"])];
    // Six matches in some 300 tracks walk; in some 1,000, one in 128 at most, they lead.
    for way in ["walked", "led"] {
        let pages = section_pages(&app, "tracks", "q=lullaby&sort=name&limit=2").await;
        assert_eq!(pages, asc, "{way}");
        let pages = section_pages(&app, "tracks", "q=lullaby&sort=name&order=desc&limit=2").await;
        assert_eq!(pages, desc, "{way}");
        let (_, page) = json(&app, "/api/v1/libraries/1/search/tracks?q=lullaby&sort=name").await;
        assert_eq!(page["total"], 6, "{way}");
        if way == "walked" {
            fill(5300..6000).await;
        }
    }
}

#[tokio::test]
async fn a_walk_that_finds_the_matches_bunched_together_gives_way_to_the_lead() {
    let (_temp, db, app) = app_with_lullabies().await;
    // Two hundred lullabies in some 25,200 tracks, more than one in 128, so they walk; but every
    // filler sorts ahead of them by name, more than a walk reads before it gives up.
    db.write(|tx| {
        for n in 0..195 {
            titled(1201 + n, &format!("Lullaby {n:03}")).insert(tx)?;
        }
        for id in 10_000..35_000 {
            titled(id, &format!("Filler {id}")).insert(tx)?;
        }
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .unwrap();
    let pages = section_pages(&app, "tracks", "q=lullaby&sort=name&limit=100").await;
    // Digits sort before letters: 000 to 194, then A to E.
    let expected: Vec<String> = (1201..1396).chain(1101..1106).map(|id| id.to_string()).collect();
    assert_eq!(pages, [expected[..100].to_vec(), expected[100..].to_vec()]);
}

/// [`catalog::app`], plus Punk Rock (33) on tracks 1002 and 1003, beside Rock (30) and the mood
/// Calm (32) on track 1001, once the index holds them.
async fn app_with_tags() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = app_with_catalog().await;
    db.write(|tx| {
        tx.execute_batch(
            "INSERT INTO tags (id, library_id, type, name, name_key, sort_key) VALUES
                 (33, 1, 'genre', 'Punk Rock', 'punk rock', CAST('punk rock' AS BLOB));
             INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) VALUES
                 (1, 1002, 33, 'Punk Rock'), (1, 1003, 33, 'Punk Rock');",
        )
    })
    .await
    .unwrap();
    eventually("the index holds the tags", async || {
        let (_, body) = json(&app, "/api/v1/libraries/1/search?q=punk").await;
        (body["sections"] != json!([])).then_some(())
    })
    .await;
    (temp, db, app)
}

/// The tags sections of a search response, as each one's type and tag IDs.
fn tag_sections(body: &Value) -> Vec<(String, Vec<String>)> {
    let sections = body["sections"].as_array().unwrap();
    let tags = sections.iter().filter(|section| section["type"] == "tags");
    tags.map(|section| {
        let ids = section["items"].as_array().unwrap();
        let ids = ids.iter().map(|item| item["tag"]["id"].as_str().unwrap().to_owned()).collect();
        (section["tagType"].as_str().unwrap().to_owned(), ids)
    })
    .collect()
}

#[tokio::test]
async fn tags_are_found_by_name_in_a_section_for_each_type() {
    let (_temp, _db, app) = app_with_tags().await;
    let genre = |ids: &[&str]| ("genre".to_owned(), ids.iter().map(|&id| id.to_owned()).collect());
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=rock").await;
    // Rock is exact; Punk Rock, on more tracks, follows it.
    assert_eq!(tag_sections(&body), [genre(&["30", "33"])]);
    let section =
        body["sections"].as_array().unwrap().iter().find(|s| s["type"] == "tags").unwrap();
    assert_eq!(section["total"], 2);
    assert_eq!(
        section["items"][0],
        json!({ "type": "tag", "tag": listed(&app, "tags", "30").await })
    );

    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=calm").await;
    assert_eq!(tag_sections(&body), [("mood".to_owned(), vec!["32".to_owned()])]);
    // A type of tag narrows the tags sections, and leaves the others be.
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=rock&tagType=mood").await;
    assert_eq!(tag_sections(&body), []);
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=rock&types=tags&tagType=genre").await;
    assert_eq!(sections(&body).len(), 1);
    assert_eq!(tag_sections(&body), [genre(&["30", "33"])]);
    // Under a tag filter, which only music carries, no tag is found.
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=rock&tagAny=30").await;
    assert_eq!(tag_sections(&body), []);
}

#[tokio::test]
async fn a_tag_is_found_as_the_caller_sees_it() {
    let (_temp, _db, app) = app_with_tags().await;
    // Every track of Rock and Calm hidden, and one of Punk Rock's two.
    for track in [1001, 1002] {
        let uri = format!("/api/v1/libraries/1/tracks/{track}/hidden");
        assert_eq!(send(app.clone(), "PUT", &uri).await.0, StatusCode::NO_CONTENT);
    }
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=rock").await;
    assert_eq!(tag_sections(&body), [("genre".to_owned(), vec!["33".to_owned()])]);
    let section =
        body["sections"].as_array().unwrap().iter().find(|s| s["type"] == "tags").unwrap();
    assert_eq!(
        (&section["total"], &section["items"][0]["tag"]["trackCount"]),
        (&json!(1), &json!(1))
    );
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=calm").await;
    assert_eq!(body["sections"], json!([]));
}

#[tokio::test]
async fn the_tags_section_pages_through_one_type() {
    let (_temp, _db, app) = app_with_tags().await;
    let base = "/api/v1/libraries/1/search/tags?q=rock";
    let (status, first) = json(&app, &format!("{base}&tagType=genre&limit=1")).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!((&first["items"][0]["tag"]["id"], &first["total"]), (&json!("30"), &json!(2)));
    let cursor = first["nextCursor"].as_str().unwrap();
    let (_, rest) = json(&app, &format!("{base}&tagType=genre&limit=1&cursor={cursor}")).await;
    assert_eq!((&rest["items"][0]["tag"]["id"], &rest["nextCursor"]), (&json!("33"), &Value::Null));
    let (_, mood) = json(&app, &format!("{base}&tagType=mood")).await;
    assert_eq!(mood["total"], 0);
    let (_, none) = json(&app, &format!("{base}&tagType=decade")).await;
    assert_eq!(none["total"], 0, "a type the library lacks");

    let (status, _, _) = send(app.clone(), "GET", base).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "the tags of no type");
    let (status, _, _) = send(app.clone(), "GET", &format!("{base}&tagType=genre&sort=name")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "tags have no sorts");
}

/// [`catalog::app`], plus user 2, who reaches library 1 with `user-token`; each has a playlist
/// called Road Trip of tracks 1001 and 1002, and the owner Liked Songs, of track 1003. Gives
/// the owner's Road Trip and Liked Songs IDs.
async fn app_with_playlists() -> (tempfile::TempDir, Router, String, String) {
    let (temp, _db, app) = app_with_catalog().await;
    {
        let conn = conn(&temp);
        add_user(&conn, 2, "user");
        add_device(&conn, 2, 2, "user-token");
        access(&conn, 2, 1).unwrap();
    }
    let uri = "/api/v1/libraries/1/playlists";
    let items = json!([{ "type": "track", "id": "1001" }, { "type": "track", "id": "1002" }]);
    let mut ids = Vec::new();
    for token in [TOKEN, "user-token"] {
        let body = json!({ "title": "Road Trip", "items": items });
        let (status, made) = call(&app, token, "POST", uri, Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
        ids.push(made["id"].as_str().unwrap().to_owned());
    }
    let like = "/api/v1/libraries/1/tracks/1003/like";
    assert_eq!(send(app.clone(), "PUT", like).await.0, StatusCode::NO_CONTENT);
    let (_, all) = json(&app, &format!("{uri}?limit=100")).await;
    let liked = all["items"].as_array().unwrap().iter().find(|p| p["kind"] == "liked").unwrap();
    (temp, app, ids[0].clone(), liked["id"].as_str().unwrap().to_owned())
}

/// The playlists section's playlist IDs, of a search for `q` by the holder of `token`.
async fn playlists_found(app: &Router, token: &str, q: &str) -> Vec<String> {
    let uri = format!("/api/v1/libraries/1/search?q={q}&types=playlists");
    let (status, body) = call(app, token, "GET", &uri, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sections = body["sections"].as_array().unwrap();
    let items = sections.iter().flat_map(|section| section["items"].as_array().unwrap());
    items.map(|item| item["playlist"]["id"].as_str().unwrap().to_owned()).collect()
}

#[tokio::test]
async fn a_search_finds_the_callers_own_playlists_and_no_one_elses() {
    let (_temp, app, road, liked) = app_with_playlists().await;
    assert_eq!(playlists_found(&app, TOKEN, "road").await, std::slice::from_ref(&road));
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=road%20trip").await;
    assert_eq!(
        body["top"],
        json!({ "type": "playlist", "playlist": listed(&app, "playlists", &road).await })
    );
    assert_eq!(playlists_found(&app, TOKEN, "liked").await, [liked]);

    // User 2 finds theirs alone, though it is named the same, and has no Liked Songs to find.
    let theirs = playlists_found(&app, "user-token", "road").await;
    assert_eq!(theirs.len(), 1);
    assert_ne!(theirs, [road]);
    assert!(playlists_found(&app, "user-token", "liked").await.is_empty());
}

#[tokio::test]
async fn a_playlist_is_found_as_its_list_shows_it() {
    let (_temp, app, road, _) = app_with_playlists().await;
    let hide = "/api/v1/libraries/1/tracks/1001/hidden";
    assert_eq!(send(app.clone(), "PUT", hide).await.0, StatusCode::NO_CONTENT);
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=road&types=playlists").await;
    let listed = listed(&app, "playlists", &road).await;
    assert_eq!(body["sections"][0]["items"][0], json!({ "type": "playlist", "playlist": listed }));
    assert_eq!(listed["trackCount"], 1, "less the track hidden");
}

#[tokio::test]
async fn the_playlists_section_pages_through_the_callers_playlists() {
    let (_temp, app, road, liked) = app_with_playlists().await;
    let uri = "/api/v1/libraries/1/playlists";
    let body = json!({ "title": "Road Songs" });
    let (status, _) = call(&app, TOKEN, "POST", uri, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED);
    let base = "/api/v1/libraries/1/search/playlists?q=road";
    // As good a match each, so by name: Road Songs, then Road Trip.
    let (_, first) = json(&app, &format!("{base}&limit=1")).await;
    assert_eq!(
        (&first["items"][0]["playlist"]["title"], &first["total"]),
        (&json!("Road Songs"), &json!(2))
    );
    let cursor = first["nextCursor"].as_str().unwrap();
    let (_, rest) = json(&app, &format!("{base}&limit=1&cursor={cursor}")).await;
    assert_eq!(
        (&rest["items"][0]["playlist"]["id"], &rest["nextCursor"]),
        (&json!(road), &Value::Null)
    );
    let (_, found) = json(&app, "/api/v1/libraries/1/search/playlists?q=liked").await;
    assert_eq!(found["items"][0]["playlist"]["id"], json!(liked));
    let (status, _, _) = send(app.clone(), "GET", &format!("{base}&sort=name")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "playlists have no sorts here");
}
