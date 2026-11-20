//! An account's devices, for admins.

use axum::Router;
use axum::http::StatusCode;
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::api::session::token_hash;
use crate::api::testing::socket::{connect_with, next_of, serve};
use crate::api::testing::{
    TOKEN, add_device, add_signed_out_device, add_user, app_with_tracks, audit, call, conn,
    router_over, state,
};
use crate::db::testing::{Device, mark_deleting};

/// The owner (user 1, [`TOKEN`]) over [`app_with_tracks`]; admins 2 (`admin-token`) and 3
/// (`admin3-token`); user 4 signed in on devices 4 (`user-token`) and 7 (`phone-token`), last
/// seen in that order, and signed out of device 8; user 5 signed in on device 5
/// (`other-token`); and user 6, being deleted.
async fn accounts() -> (tempfile::TempDir, Router) {
    let (temp, db, _app) = app_with_tracks().await;
    let app = router_over(state(&temp, db));
    let conn = conn(&temp);
    for (id, role) in [(2, "admin"), (3, "admin"), (4, "user"), (5, "user"), (6, "user")] {
        add_user(&conn, id, role);
    }
    add_device(&conn, 2, 2, "admin-token");
    add_device(&conn, 3, 3, "admin3-token");
    Device { name: "Laptop", last_seen_at: 2_000, ..Device::new(4, 4, token_hash("user-token")) }
        .insert(&conn)
        .unwrap();
    Device {
        name: "Phone",
        kind: "phone",
        platform: Some("Android"),
        last_seen_at: 3_000,
        ..Device::new(4, 7, token_hash("phone-token"))
    }
    .insert(&conn)
    .unwrap();
    add_signed_out_device(&conn, 4, 8, "old-token", "credentials_changed");
    add_device(&conn, 5, 5, "other-token");
    mark_deleting(&conn, 6).unwrap();
    (temp, app)
}

fn path(user: i64) -> String {
    format!("/api/v1/admin/users/{user}/devices")
}

/// Account `user`'s devices, each with why it was signed out.
fn devices(conn: &Connection, user: i64) -> Vec<(i64, Option<String>)> {
    conn.prepare("SELECT id, revoked_reason FROM devices WHERE user_id = ?1 ORDER BY id")
        .unwrap()
        .query_map([user], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[tokio::test]
async fn lists_the_accounts_signed_in_devices_most_recent_first_with_connected_and_no_current() {
    let (_temp, app) = accounts().await;
    let address = serve(app.clone()).await;
    // Connecting moves the phone's last seen time to now, so it stays first and is not compared.
    let _socket = connect_with(address, "phone-token").await;

    let (status, mut list) = call(&app, TOKEN, "GET", &path(4), None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"][0]["lastSeenAt"] = Value::Null;
    assert_eq!(
        list,
        json!({ "items": [
            {
                "id": "7", "name": "Phone", "type": "phone", "platform": "Android",
                "firstSeenAt": "1970-01-01T00:00:00.000Z", "lastSeenAt": null,
                "connected": true,
            },
            {
                "id": "4", "name": "Laptop", "type": "desktop",
                "firstSeenAt": "1970-01-01T00:00:00.000Z",
                "lastSeenAt": "1970-01-01T00:00:02.000Z", "connected": false,
            },
        ] })
    );
}

#[tokio::test]
async fn signing_out_one_device_ends_its_session_and_tells_the_accounts_other_devices() {
    let (temp, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut phone = connect_with(address, "phone-token").await;
    let mut laptop = connect_with(address, "user-token").await;

    let (status, body) = call(&app, TOKEN, "DELETE", &format!("{}/7", path(4)), None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));

    let revoked = json!({ "type": "sessionRevoked", "reason": "logged_out" });
    assert_eq!(next_of(&mut phone, "sessionRevoked").await, revoked);
    let (status, problem) = call(&app, "phone-token", "GET", "/api/v1/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        (&problem["code"], &problem["reason"]),
        (&json!("session_revoked"), &json!("logged_out"))
    );
    // The laptop is sent the list once without the phone.
    loop {
        let message = next_of(&mut laptop, "devices").await;
        let ids: Vec<_> = message["devices"].as_array().unwrap().iter().map(|d| &d["id"]).collect();
        if !ids.contains(&&json!("7")) {
            assert_eq!(ids, [&json!("4")]);
            break;
        }
    }

    let conn = conn(&temp);
    assert_eq!(
        devices(&conn, 4),
        [(4, None), (7, Some("logged_out".into())), (8, Some("credentials_changed".into()))]
    );
    assert_eq!(
        audit(&conn),
        [("devices_signed_out".into(), 1, 4, json!({ "deviceId": "7", "deviceName": "Phone" }))]
    );
}

#[tokio::test]
async fn the_other_devices_are_sent_the_new_list_when_the_signed_out_one_holds_no_socket() {
    let (_temp, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut laptop = connect_with(address, "user-token").await;
    let ids = |message: Value| -> Vec<Value> {
        message["devices"].as_array().unwrap().iter().map(|d| d["id"].clone()).collect()
    };
    // The list the laptop's own connection is sent, read before the sign-out, so the next list
    // can come only from the sign-out.
    assert_eq!(ids(next_of(&mut laptop, "devices").await), [json!("4"), json!("7")]);

    let (status, _) = call(&app, TOKEN, "DELETE", &format!("{}/7", path(4)), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(ids(next_of(&mut laptop, "devices").await), [json!("4")]);
}

#[tokio::test]
async fn a_device_of_another_account_or_not_signed_in_is_not_found_and_nothing_changes() {
    let (temp, app) = accounts().await;
    for device in [5, 8, 99] {
        let (status, problem) =
            call(&app, TOKEN, "DELETE", &format!("{}/{device}", path(4)), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{device}: {problem}");
    }
    let conn = conn(&temp);
    assert_eq!(devices(&conn, 5), [(5, None)]);
    assert_eq!(devices(&conn, 4)[..2], [(4, None), (7, None)]);
    assert_eq!(devices(&conn, 4)[2], (8, Some("credentials_changed".into())));
    assert!(audit(&conn).is_empty());
}

#[tokio::test]
async fn signing_out_all_leaves_no_signed_in_device_closes_every_socket_and_is_recorded() {
    let (temp, app) = accounts().await;
    let address = serve(app.clone()).await;
    let mut phone = connect_with(address, "phone-token").await;
    let mut laptop = connect_with(address, "user-token").await;

    let (status, body) = call(&app, TOKEN, "DELETE", &path(4), None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    let revoked = json!({ "type": "sessionRevoked", "reason": "logged_out" });
    assert_eq!(next_of(&mut phone, "sessionRevoked").await, revoked);
    assert_eq!(next_of(&mut laptop, "sessionRevoked").await, revoked);

    let (status, list) = call(&app, TOKEN, "GET", &path(4), None).await;
    assert_eq!((status, list), (StatusCode::OK, json!({ "items": [] })));
    let conn = conn(&temp);
    assert_eq!(
        devices(&conn, 4),
        [
            (4, Some("logged_out".into())),
            (7, Some("logged_out".into())),
            (8, Some("credentials_changed".into()))
        ]
    );
    assert_eq!(devices(&conn, 5), [(5, None)]);
    assert_eq!(audit(&conn), [("devices_signed_out".into(), 1, 4, json!({ "all": true }))]);

    // None left: nothing more is recorded.
    let (status, _) = call(&app, TOKEN, "DELETE", &path(4), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(audit(&conn).len(), 1);
}

#[tokio::test]
async fn only_an_account_the_caller_outranks_has_its_devices_listed_or_signed_out() {
    let (temp, app) = accounts().await;
    let requests = [("GET", ""), ("DELETE", ""), ("DELETE", "/2"), ("DELETE", "/3")];
    for (token, user, detail) in [
        (TOKEN, 1, "You cannot do this to your own account."),
        ("admin-token", 1, "No one can do this to the owner's account."),
        ("admin-token", 2, "You cannot do this to your own account."),
        ("admin-token", 3, "Only the owner can do this to an admin's account."),
    ] {
        for (method, suffix) in requests {
            let uri = format!("{}{suffix}", path(user));
            let (status, problem) = call(&app, token, method, &uri, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{token} {method} {uri}: {problem}");
            assert_eq!(problem["detail"], detail, "{token} {method} {uri}");
        }
    }
    for (method, suffix) in requests {
        let uri = format!("{}{suffix}", path(5));
        let (status, _) = call(&app, "user-token", method, &uri, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a user is not an admin: {method} {uri}");
    }
    for user in [6, 99] {
        for (method, suffix) in [("GET", ""), ("DELETE", ""), ("DELETE", "/5")] {
            let uri = format!("{}{suffix}", path(user));
            let (status, _) = call(&app, TOKEN, method, &uri, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
        }
    }
    let conn = conn(&temp);
    for user in [1, 2, 3, 5] {
        assert!(devices(&conn, user).iter().all(|(_, revoked)| revoked.is_none()), "{user}");
    }
    assert!(audit(&conn).is_empty());
}
