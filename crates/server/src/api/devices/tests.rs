use axum::http::StatusCode;
use serde_json::json;

use super::super::session::token_hash;
use super::super::testing::{add_device, add_signed_out_device, add_user, app, conn, json};
use crate::db::testing::Device;

#[tokio::test]
async fn lists_the_callers_signed_in_devices_most_recently_seen_first() {
    let (temp, _db, app) = app("");
    let conn = conn(&temp);
    let phone = Device {
        name: "Phone",
        kind: "phone",
        platform: Some("Android"),
        last_seen_at: 4_102_444_800_000,
        ..Device::new(1, 2, token_hash("phone"))
    };
    phone.insert(&conn).unwrap();
    add_signed_out_device(&conn, 1, 3, "signed-out", "credentials_changed");
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 4, "someone-else");

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
