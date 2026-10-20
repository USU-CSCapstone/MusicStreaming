use axum::Router;
use axum::http::StatusCode;
use jewelcase_scanner::image::describe;
use serde_json::Value;

use super::super::testing::{app, ffmpeg, owner_request, send};
use crate::db::testing::{Image, library};

/// Library 1 holds image 1, a 300×200 `cover.png`; image 2, a 64×64 picture embedded in
/// `song.mp3`; image 3, a `broken.gif` that is not an image; and image 5, recorded with no
/// content hash; image 6, a `linked.png` that links to library 2's cover; and image 7, a
/// `huge.png` that only its test writes. Library 2 holds image 4.
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
    std::fs::write(music.join("broken.gif"), b"not an image").unwrap();
    std::fs::copy(cover, other.join("cover.png")).unwrap();
    std::os::unix::fs::symlink(other.join("cover.png"), music.join("linked.png")).unwrap();
    db.write(move |tx| {
        library(tx, 1, "Music", music)?;
        library(tx, 2, "Other", other)?;
        // The cache names each rendering after its image's hash, such as `01/01-128.jpg`.
        Image { hash: vec![1], ..Image::new(1, 1, "cover.png") }.insert(tx)?;
        Image::new(2, 1, "song.mp3").insert(tx)?;
        Image { hash: vec![3], ..Image::new(3, 1, "broken.gif") }.insert(tx)?;
        Image::new(4, 2, "cover.png").insert(tx)?;
        Image { hash: Vec::new(), ..Image::new(5, 1, "cover.png") }.insert(tx)?;
        Image { hash: vec![6], ..Image::new(6, 1, "linked.png") }.insert(tx)?;
        Image { hash: vec![7], ..Image::new(7, 1, "huge.png") }.insert(tx)
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
    // JPEG's start-of-image marker, then the next marker.
    assert!(body.starts_with(&[0xFF, 0xD8, 0xFF]), "{uri}");
    let info = describe(&body).unwrap();
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
    let request = owner_request("GET", "/api/v1/libraries/1/images/1?size=64")
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
    let (temp, app) = app_with_images().await;
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
        // ffmpeg's refusal names the file; it stays in the log.
        ("/api/v1/libraries/1/images/3", StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        ("/api/v1/libraries/1/images/5", StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    ] {
        let (actual, _, body) = send(app.clone(), "GET", uri).await;
        assert_eq!(actual, status, "{uri}");
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains(temp.path().to_str().unwrap()), "{uri}: {text}");
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["code"], code, "{uri}");
    }
}

#[tokio::test]
async fn an_unreadable_image_is_an_internal_problem_that_names_no_path() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, app) = app_with_images().await;
    let cover = temp.path().join("music/cover.png");
    std::fs::set_permissions(&cover, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&cover).is_ok() {
        eprintln!("skipping: permissions do not apply to this user");
        return;
    }
    let uri = "/api/v1/libraries/1/images/1?size=64";
    let (status, _, body) = send(app.clone(), "GET", uri).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    let body = String::from_utf8(body).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["code"], "internal");
    assert!(!body.contains(temp.path().to_str().unwrap()), "{body}");
    // Not remembered as broken: readable again, it renders.
    std::fs::set_permissions(&cover, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(dimensions(&app, uri).await, (64, 43));
}

#[tokio::test]
async fn a_pipe_is_refused_without_waiting_for_a_writer() {
    let (temp, app) = app_with_images().await;
    let cover = temp.path().join("music/cover.png");
    std::fs::remove_file(&cover).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(&cover).status().unwrap().success());
    let request = send(app, "GET", "/api/v1/libraries/1/images/1?size=64");
    let answered = tokio::time::timeout(std::time::Duration::from_secs(5), request).await;
    if answered.is_err() {
        // Opening the pipe to write lets go of a reader stuck on it, so the test can end. It is
        // opened on a thread of its own, since no reader may be left to let it go.
        let pipe = cover.clone();
        std::thread::spawn(move || std::fs::OpenOptions::new().write(true).open(pipe).unwrap());
    }
    let (status, _, _) = answered.expect("answered without waiting on the pipe");
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!temp.path().join("cache/01/01-64.failed").exists(), "not remembered as broken");
    std::fs::remove_file(&cover).unwrap();
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
    std::fs::copy(music.join("cover.png"), music.join("broken.gif")).unwrap();
    let (status, _, _) = send(app, "GET", uri).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn an_image_past_the_pixel_cap_is_refused_and_not_tried_again() {
    let (temp, app) = app_with_images().await;
    // One row more than 8192², the cap; one bit a pixel keeps it quick to write.
    let huge = temp.path().join("music/huge.png");
    let huge = huge.to_str().unwrap();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=black:s=8192x8193",
        "-frames:v",
        "1",
        "-pix_fmt",
        "monob",
        huge,
    ]);
    let (status, _, _) = send(app, "GET", "/api/v1/libraries/1/images/7").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(temp.path().join("cache/07/07-512.failed").exists());
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

#[tokio::test]
async fn a_link_out_of_the_library_is_not_rendered() {
    let (temp, app) = app_with_images().await;
    let (status, _, _) = send(app, "GET", "/api/v1/libraries/1/images/6").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!temp.path().join("cache/06").exists(), "nothing rendered or remembered");
}
