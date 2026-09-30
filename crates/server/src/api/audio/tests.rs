use axum::Router;
use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::super::testing::{app, send};
use super::*;
use crate::db::libraries;

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
        libraries::create(tx, Some(1), "Music", &[&music], &[])?;
        libraries::create(tx, Some(2), "Other", &[&other], &[])?;
        tx.execute_batch(
            "INSERT INTO albums (id, library_id, title_key, artists_key, sort_key,
                                 artist_sort_key, loudness_lufs, peak_dbtp, added_at, updated_at)
             VALUES (101, 1, 'a', '', x'61', x'', -9.5, -0.25, 0, 0),
                    (201, 2, 'a', '', x'61', x'', NULL, NULL, 0, 0);
             WITH t (id, library_id, album_id, path, missing_since) AS (VALUES
                 (1, 1, 101, 'a.m4a', NULL), (2, 1, 101, 'b.m4a', 5), (3, 2, 201, 'a.m4a', NULL))
             INSERT INTO tracks (id, library_id, album_id, root_id, path, file_size, file_mtime,
                                 title, sort_key, artist_sort_key, album_sort_key, missing_since,
                                 codec, container, lossless, sample_rate_hz, bit_depth,
                                 channels, duration_us, loudness_lufs, peak_dbtp, added_at,
                                 updated_at)
             SELECT t.id, t.library_id, t.album_id, r.id, t.path, 10, 0, 'A', x'61', x'',
                    x'61', t.missing_since, 'aac', 'mp4', 0, 44100, NULL, 2, 1000000, -10.5,
                    -0.5, 0, 0
             FROM t JOIN library_roots r ON r.library_id = t.library_id;",
        )
    })
    .await
    .unwrap();
    (temp, app)
}

/// Sends a `GET` with a `Range` header.
async fn get_range(app: Router, uri: &str, range: &str) -> Response {
    let request = Request::get(uri)
        .header(header::RANGE, range)
        .body(Body::empty())
        .unwrap();
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
        assert_eq!(
            content_type.as_deref(),
            Some("application/problem+json"),
            "{uri}"
        );
        assert_eq!(json(&body)["code"], "not_found", "{uri}");
    }
}
