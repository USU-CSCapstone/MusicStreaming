use axum::Router;
use axum::http::{StatusCode, header};

use super::super::testing::catalog;
use super::super::testing::{add_device, add_user, call, conn, json, owner_request, respond};
use crate::db::Database;
use crate::db::feed::{self, Entity, Op};
use crate::db::testing::access;

/// Records `changes` of library `library` on the feed, in order.
async fn record(db: &Database, library: i64, changes: &[(Entity, i64, Op)]) {
    let changes = changes.to_vec();
    db.write(move |tx| {
        for (entity, id, op) in changes {
            feed::record(tx, library, entity, id, op, 0)?;
        }
        Ok(())
    })
    .await
    .unwrap();
}

/// Every change of `library`'s feed, a page of `limit` at a time, as `(kind, id)`; asserts the
/// walk ends where `head` says.
async fn walk(app: &Router, library: i64, limit: u32) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    let mut after = 0;
    loop {
        let uri = format!("/api/v1/libraries/{library}/changes?after={after}&limit={limit}");
        let (status, page) = json(app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let changes = page["changes"].as_array().unwrap();
        seen.extend(changes.iter().map(|c| {
            (c["kind"].as_str().unwrap().to_owned(), c["id"].as_str().unwrap().to_owned())
        }));
        let next = page["next"].as_i64().unwrap();
        assert!(next > after || next == page["head"], "no progress: {uri}");
        after = next;
        if after == page["head"] {
            return seen;
        }
    }
}

#[tokio::test]
async fn after_zero_pages_through_every_entity_exactly_once() {
    let (_temp, db, app) = catalog::app().await;
    record(
        &db,
        1,
        &[
            (Entity::Library, 1, Op::Upsert),
            (Entity::Artist, 10, Op::Upsert),
            (Entity::Album, 101, Op::Upsert),
            (Entity::Track, 1001, Op::Upsert),
            (Entity::Track, 1002, Op::Upsert),
            (Entity::Tag, 30, Op::Upsert),
        ],
    )
    .await;
    for limit in [1, 2, 3, 500] {
        let seen = walk(&app, 1, limit).await;
        let kinds = ["library", "artist", "album", "track", "track", "tag"];
        let ids = ["1", "10", "101", "1001", "1002", "30"];
        let expected: Vec<(String, String)> =
            kinds.iter().zip(ids).map(|(k, i)| ((*k).to_owned(), i.to_owned())).collect();
        assert_eq!(seen, expected, "limit {limit}");
    }
    let (_, page) = json(&app, "/api/v1/libraries/1/changes").await;
    let library = &page["changes"][0];
    assert_eq!(library["kind"], "library");
    assert_eq!(library["rows"]["libraries"][0]["id"], "1");
    assert!(library["rows"]["libraries"][0].get("name").is_none(), "no server-only column");
    let track = &page["changes"][3];
    assert_eq!(track["op"], "upsert");
    assert_eq!(track["rows"]["tracks"][0]["id"], "1001");
    assert_eq!(track["rows"]["tracks"][0]["library_id"], "1");
    assert!(track["rows"]["tracks"][0].get("path").is_none(), "no server-only column");
}

#[tokio::test]
async fn a_change_after_a_cursor_appears_once_coalesced() {
    let (_temp, db, app) = catalog::app().await;
    record(&db, 1, &[(Entity::Track, 1001, Op::Upsert), (Entity::Track, 1002, Op::Upsert)]).await;
    let (_, first) = json(&app, "/api/v1/libraries/1/changes").await;
    let cursor = first["next"].as_i64().unwrap();
    record(
        &db,
        1,
        &[
            (Entity::Track, 1001, Op::Upsert),
            (Entity::Track, 1001, Op::Upsert),
            (Entity::Artist, 10, Op::Upsert),
        ],
    )
    .await;
    let (_, page) = json(&app, &format!("/api/v1/libraries/1/changes?after={cursor}")).await;
    let changes: Vec<_> = page["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["kind"].as_str().unwrap(), c["id"].as_str().unwrap()))
        .collect();
    assert_eq!(changes, [("track", "1001"), ("artist", "10")]);
    assert_eq!(page["next"], page["head"]);
    let (_, all) = json(&app, "/api/v1/libraries/1/changes").await;
    assert_eq!(all["changes"].as_array().unwrap().len(), 3, "one row per entity");
}

#[tokio::test]
async fn a_deleted_entity_appears_as_a_tombstone() {
    let (_temp, db, app) = catalog::app().await;
    record(&db, 1, &[(Entity::Track, 1001, Op::Upsert), (Entity::Track, 1001, Op::Delete)]).await;
    let (_, page) = json(&app, "/api/v1/libraries/1/changes").await;
    let changes = page["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["op"], "delete");
    assert_eq!(changes[0]["id"], "1001");
    assert!(changes[0].get("rows").is_none());
}

#[tokio::test]
async fn another_librarys_feed_is_not_found_and_holds_none_of_ours() {
    let (temp, db, app) = catalog::app().await;
    let conn = conn(&temp);
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    access(&conn, 2, 1).unwrap();
    record(&db, 1, &[(Entity::Track, 1001, Op::Upsert)]).await;
    record(&db, 2, &[(Entity::Track, 2001, Op::Upsert), (Entity::Track, 2002, Op::Delete)]).await;

    let get = |path: &'static str| call(&app, "user-token", "GET", path, None);
    let theirs = get("/api/v1/libraries/2/changes").await;
    let missing = get("/api/v1/libraries/9/changes").await;
    assert_eq!(theirs.0, StatusCode::NOT_FOUND);
    assert_eq!(theirs, missing);

    let (status, page) = get("/api/v1/libraries/1/changes").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["changes"].as_array().unwrap().len(), 1);
    assert_eq!(page["changes"][0]["id"], "1001");
    assert_eq!(page["head"], page["changes"][0]["seq"], "head is this library's, not the server's");
    let others = [("track", "2001"), ("track", "2002")].map(|(k, i)| (k.to_owned(), i.to_owned()));
    assert_eq!(walk(&app, 2, 10).await, others);
}

#[tokio::test]
async fn bad_parameters_and_an_expired_cursor_are_refused() {
    let (_temp, db, app) = catalog::app().await;
    record(&db, 1, &[(Entity::Track, 1001, Op::Upsert)]).await;
    for query in ["limit=0", "limit=1001", "after=-1", "after=x", "other=1"] {
        let (status, _) = json(&app, &format!("/api/v1/libraries/1/changes?{query}")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{query}");
    }
    let (status, problem) = json(&app, "/api/v1/libraries/1/changes?after=999999999").await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(problem["code"], "cursor_expired");
    assert_eq!(json(&app, "/api/v1/libraries/1/changes?after=0").await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_client_that_asks_gets_a_page_gzipped() {
    let (_temp, db, app) = catalog::app().await;
    record(&db, 1, &[(Entity::Track, 1001, Op::Upsert)]).await;
    let request = owner_request("GET", "/api/v1/libraries/1/changes")
        .header(header::ACCEPT_ENCODING, "gzip")
        .body(axum::body::Body::empty())
        .unwrap();
    let (status, headers, _) = respond(app, request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_ENCODING], "gzip");
}
