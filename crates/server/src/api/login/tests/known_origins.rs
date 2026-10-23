//! At an origin an account has signed in from, the account's limit counts only that origin's
//! failures; at an origin only another account, a suspended one, or an earlier holder of the
//! name signed in from, it counts them all.

use axum::http::StatusCode;

use super::{PASSWORD, login, server};
use crate::db::Database;
use crate::db::testing::User;

const WAITS: StatusCode = StatusCode::TOO_MANY_REQUESTS;

/// Adds an active account `username` as `id`, with [`PASSWORD`].
async fn add(db: &Database, id: i64, username: &'static str) {
    db.write(move |tx| {
        let password =
            tx.query_row("SELECT password FROM users WHERE username = 'sam'", [], |r| r.get(0))?;
        User { username: username.into(), password, ..User::new(id, "user") }.insert(tx)
    })
    .await
    .unwrap();
}

/// Fails `username`'s password five times from `peer`, as many as the account gets free.
async fn lock(app: &axum::Router, peer: &str, username: &str) {
    for _ in 0..5 {
        assert_eq!(login(app, peer, username, "wrong").await.0, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn the_owner_gets_in_where_they_signed_in_before_while_locked_out_elsewhere() {
    let (_temp, db, app) = server().await;
    add(&db, 2, "kim").await;
    assert_eq!(login(&app, "2001:db8:1:2::10", "sam", PASSWORD).await.0, StatusCode::OK);
    assert_eq!(login(&app, "203.0.113.5", "kim", PASSWORD).await.0, StatusCode::OK);
    // Someone else keeps the account waiting.
    lock(&app, "198.51.100.7", "sam").await;
    assert_eq!(login(&app, "198.51.100.7", "sam", PASSWORD).await.0, WAITS);
    let elsewhere = login(&app, "203.0.113.5", "sam", PASSWORD).await.0;
    assert_eq!(elsewhere, WAITS, "signed into another account only");
    // Another address in the same /64 is the same origin.
    assert_eq!(login(&app, "2001:db8:1:2::20", "SAM", PASSWORD).await.0, StatusCode::OK);
}

#[tokio::test]
async fn failures_where_the_account_signed_in_before_lock_it_there() {
    let (_temp, _db, app) = server().await;
    assert_eq!(login(&app, "203.0.113.5", "sam", PASSWORD).await.0, StatusCode::OK);
    // The account's free failures, far fewer than the origin's.
    lock(&app, "203.0.113.5", "sam").await;
    assert_eq!(login(&app, "203.0.113.5", "sam", PASSWORD).await.0, WAITS);
    assert_eq!(login(&app, "198.51.100.7", "sam", PASSWORD).await.0, WAITS, "and elsewhere");
}

#[tokio::test]
async fn failures_elsewhere_do_not_count_where_the_account_signed_in_before() {
    let (_temp, _db, app) = server().await;
    assert_eq!(login(&app, "203.0.113.5", "sam", PASSWORD).await.0, StatusCode::OK);
    lock(&app, "198.51.100.7", "sam").await;
    for _ in 0..4 {
        assert_eq!(login(&app, "203.0.113.5", "sam", "wrong").await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(login(&app, "203.0.113.5", "sam", PASSWORD).await.0, StatusCode::OK);
}

#[tokio::test]
async fn failures_from_origins_never_signed_in_from_add_up() {
    let (_temp, _db, app) = server().await;
    for peer in ["198.51.100.7", "198.51.100.7", "198.51.100.7", "198.51.100.8", "198.51.100.8"] {
        assert_eq!(login(&app, peer, "sam", "wrong").await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(login(&app, "198.51.100.8", "sam", PASSWORD).await.0, WAITS);
}

#[tokio::test]
async fn a_suspended_account_waits_like_an_unknown_name() {
    let (_temp, db, app) = server().await;
    add(&db, 2, "kim").await;
    assert_eq!(login(&app, "203.0.113.5", "kim", PASSWORD).await.0, StatusCode::OK);
    db.write(|tx| tx.execute("UPDATE users SET status = 'suspended' WHERE username = 'kim'", []))
        .await
        .unwrap();
    for name in ["kim", "ghost"] {
        lock(&app, "198.51.100.7", name).await;
    }
    // A suspended account must not tell itself apart (`requirements/users.md` §10).
    for name in ["kim", "ghost"] {
        assert_eq!(login(&app, "203.0.113.5", name, PASSWORD).await.0, WAITS, "{name}");
    }
}

#[tokio::test]
async fn a_name_taken_again_is_not_known_where_its_last_holder_signed_in() {
    let (_temp, db, app) = server().await;
    add(&db, 2, "kim").await;
    assert_eq!(login(&app, "203.0.113.5", "kim", PASSWORD).await.0, StatusCode::OK);
    db.write(|tx| tx.execute("DELETE FROM users WHERE username = 'kim'", [])).await.unwrap();
    add(&db, 3, "kim").await;
    lock(&app, "198.51.100.7", "kim").await;
    assert_eq!(login(&app, "203.0.113.5", "kim", PASSWORD).await.0, WAITS);
}
