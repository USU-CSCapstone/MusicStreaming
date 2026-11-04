//! Each user's Liked Songs, as playlists show it (`requirements/playlists.md` §11).

use super::*;

/// The owner's Liked Songs in library 1, as listed.
async fn liked(app: &Router) -> Value {
    let (status, page) = call(app, TOKEN, "GET", URI, None).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let items = page["items"].as_array().unwrap();
    items.iter().find(|p| p["title"] == "Liked Songs").cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn it_is_listed_first_pinned_and_empty_with_no_artwork() {
    let (_temp, _db, app) = app_with_tracks().await;
    create(&app, json!({ "title": "Above it by name", "items": [{ "type": "track", "id": "1" }] }))
        .await;
    let (_, page) = call(&app, TOKEN, "GET", &format!("{URI}?sort=name"), None).await;
    let first = &page["items"][0];
    assert_eq!(
        (&first["title"], &first["pinned"], &first["trackCount"], &first["image"]),
        (&json!("Liked Songs"), &json!(true), &json!(0), &Value::Null)
    );
}

#[tokio::test]
async fn deleted_it_is_not_made_again() {
    let (_temp, db, app) = app_with_tracks().await;
    let uri = format!("{URI}/{}", liked(&app).await["id"].as_str().unwrap());
    assert_eq!(call(&app, TOKEN, "DELETE", &uri, None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(liked(&app).await, Value::Null);

    // A library made since gives the owner one there, and not again in library 1.
    db.write(|tx| library(tx, 3, "Third", "/third")).await.unwrap();
    assert_eq!(liked(&app).await, Value::Null);
    let (_, third) = call(&app, TOKEN, "GET", "/api/v1/libraries/3/playlists", None).await;
    assert_eq!(third["items"][0]["title"], "Liked Songs");
}
