use axum::Router;
use axum::http::StatusCode;
use jewelcase_scanner::image::describe;
use serde_json::Value;

use super::super::testing::{app, send};
use crate::db::libraries;

/// Runs ffmpeg to make a test file.
fn ffmpeg(args: &[&str]) {
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success());
}

/// Library 1 holds image 1, a 300×200 `cover.png`; image 2, a 64×64 picture embedded in
/// `song.mp3`; image 3, a `broken.png` that is not an image; and image 5, recorded with no
/// content hash. Library 2 holds image 4.
async fn app_with_images() -> (tempfile::TempDir, Router) {
    let (temp, db, app) = app("");
    let (music, other) = (temp.path().join("music"), temp.path().join("other"));
    std::fs::create_dir(&music).unwrap();
    std::fs::create_dir(&other).unwrap();
    let cover = music.join("cover.png");
    let cover = cover.to_str().unwrap();
    ffmpeg(&["-f", "lavfi", "-i", "color=red:s=300x200", "-frames:v", "1", cover]);
    let small = temp.path().join("small.png");
    let small = small.to_str().unwrap();
    ffmpeg(&["-f", "lavfi", "-i", "color=blue:s=64x64", "-frames:v", "1", small]);
    let song = music.join("song.mp3");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=duration=1",
        "-i",
        small,
        "-map",
        "0",
        "-map",
        "1",
        "-c:v",
        "png",
        "-disposition:v",
        "attached_pic",
        song.to_str().unwrap(),
    ]);
    std::fs::write(music.join("broken.png"), b"not an image").unwrap();
    std::fs::copy(cover, other.join("cover.png")).unwrap();
    db.write(move |tx| {
        libraries::create(tx, Some(1), "Music", &[&music], &[])?;
        libraries::create(tx, Some(2), "Other", &[&other], &[])?;
        tx.execute_batch(
            "WITH i (id, library_id, hash, path, embedded) AS (VALUES
                 (1, 1, x'01', 'cover.png', 0), (2, 1, x'02', 'song.mp3', 1),
                 (3, 1, x'03', 'broken.png', 0), (4, 2, x'04', 'cover.png', 0),
                 (5, 1, x'', 'cover.png', 0))
             INSERT INTO images (id, library_id, hash, format, width, height, root_id, path,
                                 embedded)
             SELECT i.id, i.library_id, i.hash, 'png', 1, 1, r.id, i.path, i.embedded
             FROM i JOIN library_roots r ON r.library_id = i.library_id;",
        )
    })
    .await
    .unwrap();
    (temp, app)
}

/// Fetches an image and returns its dimensions.
async fn dimensions(app: &Router, uri: &str) -> (u32, u32) {
    let (status, content_type, body) = send(app.clone(), "GET", uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}");
    assert_eq!(content_type.as_deref(), Some("image/jpeg"), "{uri}");
    let info = describe(&body).unwrap();
    assert_eq!(info.format, "jpeg", "{uri}");
    (info.width, info.height)
}

#[tokio::test]
async fn resizes_to_the_size_above_the_request() {
    let (temp, app) = app_with_images().await;
    // 100 rounds up to 128; the aspect ratio holds.
    assert_eq!(dimensions(&app, "/api/v1/libraries/1/images/1?size=100").await, (128, 85));
    // Never enlarged past the original.
    assert_eq!(dimensions(&app, "/api/v1/libraries/1/images/1?size=4096").await, (300, 200));
    // The embedded picture, at the default size.
    assert_eq!(dimensions(&app, "/api/v1/libraries/1/images/2").await, (64, 64));

    // Served from the cache once made, with nothing left behind.
    std::fs::remove_file(temp.path().join("music/cover.png")).unwrap();
    assert_eq!(dimensions(&app, "/api/v1/libraries/1/images/1?size=128").await, (128, 85));
    let cached: Vec<_> = std::fs::read_dir(temp.path().join("cache/01"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(cached.len(), 2, "{cached:?}");
    assert!(cached.contains(&"01-128.jpg".to_owned()), "{cached:?}");
}

#[tokio::test]
async fn is_cacheable_by_the_client_only() {
    let (_temp, app) = app_with_images().await;
    let request = axum::http::Request::get("/api/v1/libraries/1/images/1?size=64")
        .body(axum::body::Body::empty())
        .unwrap();
    let response = tower::ServiceExt::oneshot(app, request).await.unwrap();
    assert_eq!(
        response.headers()[axum::http::header::CACHE_CONTROL],
        "private, max-age=31536000, immutable"
    );
}

#[tokio::test]
async fn problems() {
    let (_temp, app) = app_with_images().await;
    for (uri, status, code) in [
        // Image 4 in library 1, image 1 in library 2, and one that does not exist.
        ("/api/v1/libraries/1/images/4", StatusCode::NOT_FOUND, "not_found"),
        ("/api/v1/libraries/2/images/1", StatusCode::NOT_FOUND, "not_found"),
        ("/api/v1/libraries/1/images/9", StatusCode::NOT_FOUND, "not_found"),
        (
            "/api/v1/libraries/1/images/1?size=15",
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
        ),
        (
            "/api/v1/libraries/1/images/1?size=4097",
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
        ),
        (
            "/api/v1/libraries/1/images/1?width=64",
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
        ),
        ("/api/v1/libraries/1/images/3", StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        ("/api/v1/libraries/1/images/5", StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    ] {
        let (actual, _, body) = send(app.clone(), "GET", uri).await;
        assert_eq!(actual, status, "{uri}");
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], code, "{uri}");
    }
}

#[tokio::test]
async fn a_broken_image_is_not_tried_again() {
    let (temp, app) = app_with_images().await;
    let uri = "/api/v1/libraries/1/images/3";
    let (status, _, _) = send(app.clone(), "GET", uri).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(temp.path().join("cache/03/03-512.failed").exists());
    // Readable now, but its content hash says it is the same broken image, so ffmpeg is not run.
    let music = temp.path().join("music");
    std::fs::copy(music.join("cover.png"), music.join("broken.png")).unwrap();
    let (status, _, _) = send(app, "GET", uri).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn a_missing_file_is_tried_again() {
    let (temp, app) = app_with_images().await;
    let (cover, spare) = (temp.path().join("music/cover.png"), temp.path().join("spare.png"));
    std::fs::rename(&cover, &spare).unwrap();
    let uri = "/api/v1/libraries/1/images/1?size=64";
    let (status, _, _) = send(app.clone(), "GET", uri).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    std::fs::rename(&spare, &cover).unwrap();
    assert_eq!(dimensions(&app, uri).await, (64, 43));
}
