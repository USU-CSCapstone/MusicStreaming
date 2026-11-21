//! `getMe` answers for an account that is still there, and only one.

use axum::extract::State;
use axum::http::{StatusCode, header};

use super::*;
use crate::api::testing::{add_user, app, conn};
use crate::db::testing::mark_deleting;

/// An account whose deletion began after the gate let its request through is signed out, as the
/// gate would now answer, rather than a server error over its missing name.
#[tokio::test]
async fn an_account_being_deleted_is_signed_out() {
    let (temp, db, _app) = app("");
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    mark_deleting(&conn, 2).unwrap();

    let caller = Caller { user: 2, device: 2, admin: false, owner: false };
    let response = get(State(db), caller).await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["code"], "unauthenticated");
}
