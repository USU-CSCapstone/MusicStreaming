//! The whole path from files on disk to the API: the scanner writes through [`SqliteStore`], and
//! the handlers read what it wrote. Every other API test reads hand-written rows.

use axum::Router;
use axum::http::StatusCode;
use jewelcase_scanner::describe_image;
use jewelcase_scanner::testing::{library, make_album, scan_library};
use serde_json::Value;

use super::super::testing::{app, ffmpeg, json, send};
use crate::db::{SqliteStore, libraries};

/// Fails if `value` has a `path` field at any depth (`requirements/general.md` §3.7).
fn assert_no_path_field(uri: &str, value: &Value) {
    match value {
        Value::Object(fields) => {
            assert!(!fields.contains_key("path"), "{uri} has a path field: {value}");
            fields.values().for_each(|field| assert_no_path_field(uri, field));
        }
        Value::Array(items) => items.iter().for_each(|item| assert_no_path_field(uri, item)),
        _ => {}
    }
}

/// The JSON at `uri`, which must answer 200 and carry no storage path: no `path` field, no part
/// of the library's location on disk, no folder, and no file name with or without its extension.
async fn get_json(app: &Router, uri: &str, root: &str) -> Value {
    let (status, body) = json(app, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    assert_no_path_field(uri, &body);
    let text = body.to_string();
    for leak in [root, "Nevermind/", "01 - Track", ".wav", ".lrc", "cover."] {
        assert!(!text.contains(leak), "{uri} names {leak}: {text}");
    }
    body
}

/// A scanned album with a cover and a lyrics file answers every browse, lyrics, image and audio
/// request with what the scanner read from its files.
#[tokio::test]
async fn a_scanned_album_reads_back_through_the_api() {
    let (temp, db, app) = app("");
    let root = temp.path().join("music");
    make_album(&root, "Nirvana", "Nevermind", 2);
    let album_dir = root.join("Nirvana/Nevermind");
    let cover = album_dir.join("cover.png");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=red:s=300x200",
        "-frames:v",
        "1",
        cover.to_str().unwrap(),
    ]);
    std::fs::write(album_dir.join("01 - Track 1.lrc"), "[00:01.00]hello\n").unwrap();
    let created = root.clone();
    db.write(move |tx| libraries::create(tx, Some(1), "Music", &[&created], &[])).await.unwrap();
    let lib = library(&root);
    let scanned = db.clone();
    // The store writes with `write_blocking`, which cannot wait inside the runtime.
    let scan = tokio::task::spawn_blocking(move || {
        scan_library(&SqliteStore::new(scanned).unwrap(), &lib)
    })
    .await
    .unwrap();
    assert_eq!(scan.progress.added, 2, "{:?}", scan.progress);
    let root = root.to_str().unwrap();
    let base = "/api/v1/libraries/1";

    let tracks = get_json(&app, &format!("{base}/tracks"), root).await;
    let tracks = tracks["items"].as_array().unwrap();
    let titles: Vec<&str> = tracks.iter().map(|track| track["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Track 1", "Track 2"]);
    assert!(tracks.iter().all(|track| track["availability"] == "available"), "{tracks:?}");
    let track = tracks[0]["id"].as_str().unwrap();
    let got = get_json(&app, &format!("{base}/tracks/{track}"), root).await;
    assert_eq!(got["title"], "Track 1");
    assert_eq!(got["album"]["title"], "Nevermind");
    assert_eq!(got["artists"][0]["name"], "Nirvana");
    assert_eq!(got["lyrics"], "synced");

    let albums = get_json(&app, &format!("{base}/albums"), root).await;
    assert_eq!(albums["items"].as_array().unwrap().len(), 1, "{albums}");
    let album = albums["items"][0]["id"].as_str().unwrap();
    let got = get_json(&app, &format!("{base}/albums/{album}"), root).await;
    assert_eq!(got["title"], "Nevermind");
    assert_eq!(got["trackCount"], 2);
    assert_eq!(got["availability"], "available");
    let image = got["image"]["id"].as_str().unwrap();

    let artists = get_json(&app, &format!("{base}/artists"), root).await;
    assert_eq!(artists["items"].as_array().unwrap().len(), 1, "{artists}");
    let artist = artists["items"][0]["id"].as_str().unwrap();
    let got = get_json(&app, &format!("{base}/artists/{artist}"), root).await;
    assert_eq!(got["name"], "Nirvana");
    assert_eq!(got["trackCount"], 2);

    let lyrics = get_json(&app, &format!("{base}/tracks/{track}/lyrics"), root).await;
    assert_eq!(lyrics["kind"], "synced");
    assert_eq!(lyrics["lines"][0]["text"], "hello", "{lyrics}");

    let (status, content_type, body) =
        send(app.clone(), "GET", &format!("{base}/images/{image}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("image/jpeg"));
    // JPEG's start-of-image marker, then the next marker.
    assert!(body.starts_with(&[0xFF, 0xD8, 0xFF]));
    let info = describe_image(&body).unwrap();
    assert_eq!((info.width, info.height), (300, 200));

    let (status, content_type, body) =
        send(app.clone(), "GET", &format!("{base}/tracks/{track}/audio?variant=original")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("audio/wav"));
    let file = std::fs::read(album_dir.join("01 - Track 1.wav")).unwrap();
    assert_eq!(body, file);
}
