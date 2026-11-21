use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use rusqlite::Connection;
use serde_json::{Value, json};

use super::super::Id;
use super::super::extract::Path;
use super::super::scope::Caller;
use super::super::session::token_hash;
use super::super::testing::socket::{connect_with, next_of, serve};
use super::super::testing::{
    TOKEN, add_device, add_signed_out_device, add_user, app, app_with_tracks, call, conn, json,
    router_over, state,
};
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

/// The owner (user 1, device 1, [`TOKEN`]) signed in also on devices 2 (`phone-token`) and 3
/// (`tablet-token`), and signed out of device 4; and user 2 signed in on device 5
/// (`other-token`).
async fn owner_on_three_devices() -> (tempfile::TempDir, Router) {
    let (temp, db, _app) = app_with_tracks().await;
    let app = router_over(state(&temp, db));
    let conn = conn(&temp);
    add_device(&conn, 1, 2, "phone-token");
    add_device(&conn, 1, 3, "tablet-token");
    add_signed_out_device(&conn, 1, 4, "old-token", "credentials_changed");
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 5, "other-token");
    (temp, app)
}

/// Every device with why it was signed out.
fn revoked(conn: &Connection) -> Vec<(i64, Option<String>)> {
    conn.prepare("SELECT id, revoked_reason FROM devices ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn audit_rows(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM admin_actions", [], |row| row.get(0)).unwrap()
}

fn ids(message: &Value) -> Vec<Value> {
    message["devices"].as_array().unwrap().iter().map(|d| d["id"].clone()).collect()
}

#[tokio::test]
async fn signing_out_another_device_ends_its_session_and_tells_the_remaining_devices() {
    let (temp, app) = owner_on_three_devices().await;
    let address = serve(app.clone()).await;
    let mut phone = connect_with(address, "phone-token").await;
    let mut tablet = connect_with(address, "tablet-token").await;
    // The list the tablet's own connection is sent, read first so the next can come only from
    // the sign-out.
    let first = next_of(&mut tablet, "devices").await;
    assert!(ids(&first).contains(&json!("2")), "{first}");

    let (status, body) = call(&app, TOKEN, "DELETE", "/api/v1/me/devices/2", None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));

    let revoked_message = json!({ "type": "sessionRevoked", "reason": "logged_out" });
    assert_eq!(next_of(&mut phone, "sessionRevoked").await, revoked_message);
    let (status, problem) = call(&app, "phone-token", "GET", "/api/v1/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        (&problem["code"], &problem["reason"]),
        (&json!("session_revoked"), &json!("logged_out"))
    );
    // Lists sent as the sockets connected may still arrive; the first without the phone is sent
    // by the sign-out or by the phone's closing connection.
    loop {
        let list = ids(&next_of(&mut tablet, "devices").await);
        if !list.contains(&json!("2")) {
            assert_eq!(list, [json!("1"), json!("3")]);
            break;
        }
    }
    let conn = conn(&temp);
    assert_eq!(revoked(&conn)[..3], [(1, None), (2, Some("logged_out".into())), (3, None)]);
    assert_eq!(audit_rows(&conn), 0);
}

#[tokio::test]
async fn a_device_of_another_account_or_not_signed_in_is_not_found_and_nothing_changes() {
    let (temp, app) = owner_on_three_devices().await;
    for device in [5, 4, 99] {
        let (status, problem) =
            call(&app, TOKEN, "DELETE", &format!("/api/v1/me/devices/{device}"), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{device}: {problem}");
    }
    let (status, _) = call(&app, "other-token", "GET", "/api/v1/me", None).await;
    assert_eq!(status, StatusCode::OK);
    let conn = conn(&temp);
    assert_eq!(
        revoked(&conn),
        [(1, None), (2, None), (3, None), (4, Some("credentials_changed".into())), (5, None)]
    );
}

#[tokio::test]
async fn the_calling_device_is_told_to_log_out_and_stays_signed_in() {
    let (temp, app) = owner_on_three_devices().await;
    let (status, problem) = call(&app, TOKEN, "DELETE", "/api/v1/me/devices/1", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["detail"], "This is the device you are using. Log out instead.");
    assert_eq!(revoked(&conn(&temp))[0], (1, None));
}

#[tokio::test]
async fn signing_out_the_others_leaves_only_the_calling_device_and_records_nothing() {
    let (temp, app) = owner_on_three_devices().await;
    let address = serve(app.clone()).await;
    // No signed-out device holds a socket, so the list without them can come only from the
    // sign-out, not from a closing connection.
    let mut caller = connect_with(address, TOKEN).await;
    let first = next_of(&mut caller, "devices").await;
    assert!(ids(&first).contains(&json!("2")), "{first}");

    let (status, body) = call(&app, TOKEN, "DELETE", "/api/v1/me/devices", None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));

    loop {
        let list = ids(&next_of(&mut caller, "devices").await);
        if !list.contains(&json!("2")) {
            assert_eq!(list, [json!("1")]);
            break;
        }
    }
    let (status, _) = call(&app, TOKEN, "GET", "/api/v1/me", None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "tablet-token", "GET", "/api/v1/me", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let conn = conn(&temp);
    assert_eq!(
        revoked(&conn),
        [
            (1, None),
            (2, Some("logged_out".into())),
            (3, Some("logged_out".into())),
            (4, Some("credentials_changed".into())),
            (5, None)
        ]
    );
    assert_eq!(audit_rows(&conn), 0);
}

#[tokio::test]
async fn a_device_signed_out_since_the_gate_signs_out_nothing() {
    for which in ["one", "others"] {
        let (temp, db, _app) = app_with_tracks().await;
        let state = state(&temp, db);
        let conn = conn(&temp);
        add_device(&conn, 1, 2, "phone-token");
        add_device(&conn, 1, 3, "tablet-token");
        conn.execute("UPDATE devices SET revoked_reason = 'logged_out' WHERE id = 2", []).unwrap();
        let before = revoked(&conn);

        let caller = Caller { user: 1, device: 2, admin: true, owner: true };
        let (db, hub) = (State(state.db.clone()), State(state.hub.clone()));
        let answer = match which {
            "one" => super::sign_out_mine(db, hub, caller, Path(Id(3))).await,
            _ => super::sign_out_mine_others(db, hub, caller).await,
        };
        let Err(problem) = answer else { panic!("{which}: devices were signed out") };
        let problem = serde_json::to_value(problem).unwrap();
        assert_eq!(problem["status"], 401, "{which}");
        assert_eq!(
            (&problem["code"], &problem["reason"]),
            (&json!("session_revoked"), &json!("logged_out")),
            "{which}"
        );
        assert_eq!(revoked(&conn), before, "{which}: no device is signed out");
    }
}
