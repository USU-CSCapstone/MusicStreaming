use axum::Router;
use axum::http::StatusCode;
use serde_json::{Value, json};

use super::*;
use crate::api::testing::{catalog, json, send};

/// [`catalog::app`] with lyrics for library 1's tracks: Come Together (1001), plain, with a label
/// and a blank line; Something (1002), timed; Her Majesty (1003) and Bonus (1004), which hold
/// "other side" in order, and Idioteque (1005), which holds it out of order in a shorter line and
/// sings Something's title; and library 2's Elsewhere (2001).
async fn app_with_lyrics() -> (tempfile::TempDir, Arc<Database>, Router) {
    let (temp, db, app) = catalog::app().await;
    db.write(|tx| {
        tx.execute_batch(
            r#"INSERT INTO track_lyrics (library_id, track_id, plain, synced) VALUES
                (1, 1001, '[Verse]
Hello from the other side

I must have called a thousand times
[Chorus]', NULL),
                (1, 1002, NULL, '[{"startMs":0,"text":"Say it ain''t so"},
                                  {"startMs":2500,"text":"I will not go"}]'),
                (1, 1003, 'Lights off
Other side of the world', NULL),
                (1, 1004, 'Over to the other side we go', NULL),
                (1, 1005, 'Side the other way
Something in the way', NULL),
                (2, 2001, 'Hello from elsewhere', NULL);"#,
        )
    })
    .await
    .unwrap();
    (temp, db, app)
}

/// The lyrics section's matches of `uri`'s search, each as its track's ID and its line's text.
fn found(body: &Value) -> Vec<(String, String)> {
    let section = body["sections"].as_array().unwrap().iter().find(|s| s["type"] == "lyrics");
    let items =
        section.map_or_else(Vec::new, |section| section["items"].as_array().unwrap().clone());
    items.iter().map(|item| matched(&item["lyric"])).collect()
}

/// A `LyricMatch`'s track ID and line text.
fn matched(lyric: &Value) -> (String, String) {
    (
        lyric["track"]["id"].as_str().unwrap().to_owned(),
        lyric["line"]["text"].as_str().unwrap().to_owned(),
    )
}

/// `(id, text)`, owned.
fn line(id: &str, text: &str) -> (String, String) {
    (id.to_owned(), text.to_owned())
}

/// The item with `id` from library 1's tracks, as the list shows it.
async fn listed(app: &Router, id: &str) -> Value {
    let (_, page) = json(app, "/api/v1/libraries/1/tracks?limit=1000").await;
    page["items"].as_array().unwrap().iter().find(|item| item["id"] == id).unwrap().clone()
}

#[tokio::test]
async fn a_line_finds_its_song_shown_with_the_lines_around_it() {
    let (_temp, _db, app) = app_with_lyrics().await;
    let (status, body) =
        json(&app, "/api/v1/libraries/1/search?q=hello%20from%20the%20othr&types=lyrics").await;
    assert_eq!(status, StatusCode::OK);
    let lyric = json!({
        "track": listed(&app, "1001").await,
        "line": { "text": "Hello from the other side", "startMs": null },
        // The label before it is not a line, and the blank after it is passed over.
        "before": [],
        "after": ["I must have called a thousand times"],
    });
    assert_eq!(
        body["sections"],
        json!([{ "type": "lyrics", "items": [{ "type": "lyric", "lyric": lyric }], "total": 1 }])
    );
    assert_eq!(body["top"], json!({ "type": "lyric", "lyric": lyric }), "the only section");

    // A timed line says when it starts.
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=will%20not%20go").await;
    let lyric = &body["sections"][0]["items"][0]["lyric"];
    assert_eq!(lyric["line"], json!({ "text": "I will not go", "startMs": 2500 }));
    assert_eq!((&lyric["before"], &lyric["after"]), (&json!(["Say it ain't so"]), &json!([])));
}

#[tokio::test]
async fn lyrics_are_searched_by_default_after_the_names() {
    let (_temp, _db, app) = app_with_lyrics().await;
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=something").await;
    let kinds: Vec<&str> =
        body["sections"].as_array().unwrap().iter().map(|s| s["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["tracks", "lyrics"]);
    assert_eq!(body["top"]["track"]["id"], "1002", "the title, not the lyric");
    assert_eq!(found(&body), [line("1005", "Something in the way")]);
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=called").await;
    assert_eq!(found(&body), [line("1001", "I must have called a thousand times")]);
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=bonus%20go").await;
    assert_eq!(found(&body), [], "both words in one line");
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=hello").await;
    assert_eq!(found(&body), [line("1001", "Hello from the other side")], "library 1's only");
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=hello&types=tracks").await;
    assert_eq!(found(&body), [], "not asked for");
}

#[tokio::test]
async fn a_query_of_fewer_than_three_characters_searches_no_lyrics() {
    let (_temp, _db, app) = app_with_lyrics().await;
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=go&types=lyrics").await;
    assert_eq!(body["sections"], json!([]));
    let (_, page) = json(&app, "/api/v1/libraries/1/search/lyrics?q=s%20o").await;
    assert_eq!(page, json!({ "query": "s o", "items": [], "nextCursor": null, "total": 0 }));
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=wil&types=lyrics").await;
    assert_eq!(found(&body), [line("1002", "I will not go")]);
}

#[tokio::test]
async fn lines_holding_the_query_in_order_come_first() {
    let (_temp, _db, app) = app_with_lyrics().await;
    // Idioteque's line is the shortest, but holds the words out of order.
    let (_, body) = json(&app, "/api/v1/libraries/1/search?q=other%20side&types=lyrics").await;
    assert_eq!(
        found(&body),
        [
            line("1001", "Hello from the other side"),
            line("1003", "Other side of the world"),
            line("1004", "Over to the other side we go"),
            line("1005", "Side the other way"),
        ]
    );
    assert_eq!(body["sections"][0]["total"], 4);
}

/// The lyrics section's pages for `query`, `limit` at a time, each as its matches' track IDs.
async fn pages(app: &Router, query: &str, limit: u32) -> Vec<Vec<String>> {
    let base = format!("/api/v1/libraries/1/search/lyrics?q={query}&limit={limit}");
    let mut pages = Vec::new();
    let mut uri = base.clone();
    loop {
        let (status, page) = json(app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        let items = page["items"].as_array().unwrap();
        pages.push(items.iter().map(|item| matched(&item["lyric"]).0).collect());
        match page["nextCursor"].as_str() {
            Some(cursor) => uri = format!("{base}&cursor={cursor}"),
            None => return pages,
        }
    }
}

#[tokio::test]
async fn the_section_pages_through_every_match_in_order() {
    let (_temp, _db, app) = app_with_lyrics().await;
    let ids = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
    // Across the lifted matches and the rest.
    assert_eq!(
        pages(&app, "other%20side", 1).await,
        [ids(&["1001"]), ids(&["1003"]), ids(&["1004"]), ids(&["1005"])]
    );
    assert_eq!(
        pages(&app, "other%20side", 3).await,
        [ids(&["1001", "1003", "1004"]), ids(&["1005"])]
    );
    assert_eq!(pages(&app, "sid", 2).await, [ids(&["1005", "1001"]), ids(&["1003", "1004"])]);
    let (_, page) = json(&app, "/api/v1/libraries/1/search/lyrics?q=other%20side&limit=2").await;
    assert_eq!(page["total"], 4);
    // Another section's cursor is not ours.
    let cursor = crate::api::cursor::encode("search.lyrics", &[rusqlite::types::Value::Integer(2)]);
    let uri = format!("/api/v1/libraries/1/search/lyrics?q=side&cursor={cursor}");
    assert_eq!(send(app.clone(), "GET", &uri).await.0, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn what_the_caller_hid_and_what_is_beyond_the_search_is_left_out() {
    let (_temp, _db, app) = app_with_lyrics().await;
    let hide = "/api/v1/libraries/1/tracks/1003/hidden";
    assert_eq!(send(app.clone(), "PUT", hide).await.0, StatusCode::NO_CONTENT);
    let search =
        |rest: &str| format!("/api/v1/libraries/1/search?q=other%20side&types=lyrics{rest}");
    let (_, body) = json(&app, &search("")).await;
    assert_eq!(
        found(&body).iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["1001", "1004", "1005"]
    );
    // Within Abbey Road (101), which Idioteque is not on.
    let (_, body) = json(&app, &search("&within=album:101")).await;
    assert_eq!(
        found(&body).iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["1001", "1004"]
    );
    let (_, page) =
        json(&app, "/api/v1/libraries/1/search/lyrics?q=other%20side&within=album:101").await;
    assert_eq!(page["total"], 2);
    // Only Come Together is Rock (30).
    let (_, body) = json(&app, &search("&tagAll=30")).await;
    assert_eq!(found(&body), [line("1001", "Hello from the other side")]);
}

#[tokio::test]
async fn a_search_matches_its_words_once_while_the_index_stands() {
    let (_temp, db, _app) = app_with_lyrics().await;
    let indexes = Indexes::default();
    let index = indexes.lyrics(&db, 1).await.unwrap();
    let first = indexes.lyric_matches.get(1, index.clone(), "other side").await.unwrap();
    let again = indexes.lyric_matches.get(1, index.clone(), "Other  SIDE!").await.unwrap();
    assert!(Arc::ptr_eq(&first, &again), "the same words");
    let other = indexes.lyric_matches.get(1, index, "other sid").await.unwrap();
    assert!(!Arc::ptr_eq(&first, &other));
    let rebuilt = Arc::new(LyricIndex::build([(1001, vec!["the other side"])]));
    let anew = indexes.lyric_matches.get(1, rebuilt, "other side").await.unwrap();
    assert_eq!(anew.len(), 1, "matched anew in the index rebuilt");
}

#[test]
fn stored_lyrics_are_lines_in_either_form() {
    let synced = |start_ms, text: &str| SyncedLine { start_ms, text: text.to_owned() };
    let stored = Stored::new(None, Some(vec![synced(0, "one"), synced(900, "")]));
    assert_eq!(
        (stored.lines, stored.starts),
        (vec!["one".to_owned(), String::new()], Some(vec![0, 900]))
    );
    let stored = Stored::new(Some("one\n\n[Chorus]\ntwo".to_owned()), None);
    assert_eq!(stored.lines, ["one", "", "[Chorus]", "two"]);
    assert_eq!(stored.starts, None);
    assert_eq!(
        (stored.beside(0, |at| at.checked_add(1)), stored.beside(3, |at| at.checked_sub(1))),
        (vec!["two".to_owned()], vec!["one".to_owned()])
    );
    assert!(stored.beside(0, |at| at.checked_sub(1)).is_empty());
    assert!(Stored::new(None, None).lines.is_empty());
}
