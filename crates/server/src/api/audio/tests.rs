use axum::Router;
use axum::body::to_bytes;
use axum::http::StatusCode;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::super::testing::{app, owner_request, send};
use super::*;
use crate::db::libraries;
use crate::db::testing::{Album, Track, library};

const AUDIO: &[u8] = b"0123456789";

/// Library 1 holds track 1, AAC on disk as `a.m4a`, and track 2, missing since a scan.
/// Library 2 holds track 3.
async fn app_with_fixture() -> (tempfile::TempDir, Router) {
    let (temp, db, app) = app("");
    let (music, other) = (temp.path().join("music"), temp.path().join("other"));
    for dir in [&music, &other] {
        std::fs::create_dir(dir).unwrap();
        std::fs::write(dir.join("a.m4a"), AUDIO).unwrap();
    }
    db.write(move |tx| {
        library(tx, 1, "Music", music)?;
        library(tx, 2, "Other", other)?;
        Album { loudness_lufs: Some(-9.5), peak_dbtp: Some(-0.25), ..Album::new(101, 1) }
            .insert(tx)?;
        Album::new(201, 2).insert(tx)?;
        for (id, library, album, path, missing_since) in
            [(1, 1, 101, "a.m4a", None), (2, 1, 101, "b.m4a", Some(5)), (3, 2, 201, "a.m4a", None)]
        {
            let track = Track {
                path: path.into(),
                missing_since,
                codec: "aac",
                container: "mp4",
                lossless: false,
                loudness_lufs: Some(-10.5),
                peak_dbtp: Some(-0.5),
                ..Track::new(id, library, album)
            };
            track.insert(tx)?;
        }
        Ok(())
    })
    .await
    .unwrap();
    (temp, app)
}

/// Sends a `GET` with a `Range` header.
async fn get_range(app: Router, uri: &str, range: &str) -> Response {
    let request =
        owner_request("GET", uri).header(header::RANGE, range).body(Body::empty()).unwrap();
    app.oneshot(request).await.unwrap()
}

fn json(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap()
}

#[tokio::test]
async fn playback_delivers_the_original() {
    let (_temp, app) = app_with_fixture().await;
    let (status, _, body) = send(
        app.clone(),
        "GET",
        "/api/v1/libraries/1/tracks/1/playback?quality=original&purpose=download&codecs=opus,aac,mp4",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json(&body),
        json!({
            "variant": "original",
            "delivery": "direct",
            "codec": "aac",
            "container": "mp4",
            "bitrateKbps": null,
            "sampleRateHz": 44100,
            "bitDepth": null,
            "channels": 2,
            "sizeBytes": 10,
            "transport": "progressive",
            "loudness": {
                "trackLufs": -10.5,
                "trackPeakDbtp": -0.5,
                "albumLufs": -9.5,
                "albumPeakDbtp": -0.25,
            },
            "gapless": null,
        })
    );
}

#[tokio::test]
async fn playback_refuses_what_needs_a_transcode() {
    let (_temp, app) = app_with_fixture().await;
    for query in [
        "quality=high",
        "quality=original&codecs=mp3,opus",
        // The codec alone is not enough: the container must play too, and the other way round.
        "quality=original&codecs=aac,ogg",
        "quality=original&codecs=mp4",
        // Not a request that needs a transcode, but malformed.
        "purpose=stream",
        "quality=best",
        "quality=original&purpose=radio",
        "quality=original&extra=1",
    ] {
        let uri = format!("/api/v1/libraries/1/tracks/1/playback?{query}");
        let (status, _, body) = send(app.clone(), "GET", &uri).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
        assert_eq!(json(&body)["code"], "validation_failed", "{query}");
    }
}

#[tokio::test]
async fn audio_serves_the_file_and_its_ranges() {
    let (_temp, app) = app_with_fixture().await;
    let uri = "/api/v1/libraries/1/tracks/1/audio?variant=original";
    let (status, content_type, body) = send(app.clone(), "GET", uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type.as_deref(), Some("audio/mp4"));
    assert_eq!(body, AUDIO);

    let response = get_range(app.clone(), uri, "bytes=2-5").await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    let headers = response.headers();
    assert_eq!(headers[header::CONTENT_TYPE], "audio/mp4");
    assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
    assert_eq!(headers[header::CONTENT_RANGE], "bytes 2-5/10");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"2345");

    let response = get_range(app.clone(), uri, "bytes=20-").await;
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes */10");

    let (status, _, body) = send(app, "HEAD", uri).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());
}

#[tokio::test]
async fn only_the_callers_library_is_reachable() {
    let (temp, app) = app_with_fixture().await;
    std::fs::remove_file(temp.path().join("other/a.m4a")).unwrap();
    // Track 3 in library 1, track 1 in library 2, track 2 missing, and an unknown variant.
    for uri in [
        "/api/v1/libraries/1/tracks/3/playback?quality=original",
        "/api/v1/libraries/2/tracks/1/playback?quality=original",
        "/api/v1/libraries/1/tracks/2/playback?quality=original",
        "/api/v1/libraries/1/tracks/3/audio?variant=original",
        "/api/v1/libraries/2/tracks/1/audio?variant=original",
        "/api/v1/libraries/1/tracks/2/audio?variant=original",
        "/api/v1/libraries/1/tracks/1/audio?variant=high",
        // The file is gone since the last scan.
        "/api/v1/libraries/2/tracks/3/audio?variant=original",
    ] {
        let (status, content_type, body) = send(app.clone(), "GET", uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(content_type.as_deref(), Some("application/problem+json"), "{uri}");
        assert_eq!(json(&body)["code"], "not_found", "{uri}");
    }
}

#[tokio::test]
async fn an_unreadable_file_is_an_internal_problem_that_names_no_path() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, app) = app_with_fixture().await;
    let file = temp.path().join("music/a.m4a");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&file).is_ok() {
        eprintln!("skipping: permissions do not apply to this user");
        return;
    }
    let uri = "/api/v1/libraries/1/tracks/1/audio?variant=original";
    let (status, content_type, body) = send(app, "GET", uri).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(content_type.as_deref(), Some("application/problem+json"));
    let body = String::from_utf8(body).unwrap();
    assert_eq!(json(body.as_bytes())["code"], "internal");
    assert!(!body.contains(temp.path().to_str().unwrap()), "{body}");
}

#[tokio::test]
async fn a_file_reached_through_a_link_is_not_served() {
    use std::os::unix::fs::symlink;
    let (temp, db, app) = app("");
    let dir = |name| temp.path().join(name);
    // Library 1's root is itself reached through a link, which is allowed.
    let (stored, music, more, other) = (dir("stored"), dir("music"), dir("more"), dir("other"));
    for dir in [&stored, &more, &other] {
        std::fs::create_dir(dir).unwrap();
    }
    symlink(&stored, &music).unwrap();
    std::fs::write(music.join("a.m4a"), AUDIO).unwrap();
    std::fs::write(more.join("b.m4a"), AUDIO).unwrap();
    std::fs::write(other.join("c.m4a"), b"library two").unwrap();
    std::fs::write(temp.path().join("secret.txt"), b"no library").unwrap();
    // Links to library 1's second root and into library 2.
    symlink(more.join("b.m4a"), music.join("second-root.m4a")).unwrap();
    symlink(other.join("c.m4a"), music.join("cross.m4a")).unwrap();
    db.write(move |tx| {
        library(tx, 1, "Music", music)?;
        library(tx, 2, "Other", other)?;
        Album::new(101, 1).insert(tx)?;
        for (id, path) in [(1, "a.m4a"), (2, "second-root.m4a"), (3, "cross.m4a")] {
            Track { path: path.into(), container: "mp4", ..Track::new(id, 1, 101) }.insert(tx)?;
        }
        // Added after the tracks, which go on the library's only root.
        libraries::add_root(tx, 1, &more).map(drop)
    })
    .await
    .unwrap();
    let uri = |id| format!("/api/v1/libraries/1/tracks/{id}/audio?variant=original");
    let (status, _, body) = send(app.clone(), "GET", &uri(1)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, AUDIO);

    // Track 1 replaced by a link out of the library after the scan saw it.
    let a = temp.path().join("music/a.m4a");
    std::fs::remove_file(&a).unwrap();
    symlink(temp.path().join("secret.txt"), &a).unwrap();
    for id in [1, 2, 3] {
        let (status, _, body) = send(app.clone(), "GET", &uri(id)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "track {id}");
        assert_eq!(json(&body)["code"], "not_found", "track {id}");
    }
}

#[tokio::test]
async fn a_removed_root_is_not_served() {
    let (temp, app) = app_with_fixture().await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    conn.execute("UPDATE library_roots SET removed_at = 1 WHERE library_id = 1", []).unwrap();
    let (status, _, _) =
        send(app, "GET", "/api/v1/libraries/1/tracks/1/audio?variant=original").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
