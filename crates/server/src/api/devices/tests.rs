use axum::http::StatusCode;
use serde_json::json;

use super::super::testing::{add_device, add_user, app, json};

#[tokio::test]
async fn lists_the_callers_signed_in_devices_most_recently_seen_first() {
    let (temp, _db, app) = app("");
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_device(&conn, 1, 2, "phone");
    add_device(&conn, 1, 3, "logged-out");
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 4, "someone-else");
    conn.execute_batch(
        "UPDATE devices SET name = 'Phone', type = 'phone', platform = 'Android', \
             last_seen_at = 4102444800000 WHERE id = 2;
         UPDATE devices SET token_hash = NULL, revoked_reason = 'credentials_changed' WHERE id = 3;",
    )
    .unwrap();

    let (status, list) = json(&app, "/api/v1/me/devices").await;
    assert_eq!(status, StatusCode::OK);
    // The calling device's own time moves as it calls, so only the phone's is checked.
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{list}");
    assert_eq!(
        items[0],
        json!({
            "id": "2", "name": "Phone", "type": "phone", "platform": "Android",
            "firstSeenAt": "1970-01-01T00:00:00.000Z", "lastSeenAt": "2100-01-01T00:00:00.000Z",
            "connected": false, "current": false,
        })
    );
    assert_eq!((&items[1]["id"], &items[1]["current"]), (&json!("1"), &json!(true)));
}
