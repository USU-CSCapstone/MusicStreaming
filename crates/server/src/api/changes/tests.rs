use axum::Router;
use axum::http::{StatusCode, header};

use super::super::testing::catalog;
use std::sync::Arc;

use super::super::testing::{
    add_device, add_user, call, conn, json, owner_request, respond, router_over, state,
};
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
    let (mut after, mut epoch) = (0, String::new());
    loop {
        let uri = format!(
            "/api/v1/libraries/{library}/changes?after={after}&epoch={epoch}&limit={limit}"
        );
        let (status, page) = json(app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        epoch = page["epoch"].as_str().unwrap().to_owned();
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
    let cursor = format!("{}&epoch={}", first["next"], first["epoch"].as_str().unwrap());
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
    assert_eq!(page["head"], 1, "a position of library 1's own, before library 2's changes");
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
    let epoch = db.epoch();
    let gone = [
        format!("after=2&epoch={epoch}"),
        "after=1".to_owned(),
        format!("after=1&epoch={}", epoch ^ 1),
        "after=1&epoch=x".to_owned(),
    ];
    for query in gone {
        let (status, problem) = json(&app, &format!("/api/v1/libraries/1/changes?{query}")).await;
        assert_eq!(
            (status, &problem["code"]),
            (StatusCode::GONE, &"cursor_expired".into()),
            "{query}"
        );
    }
    for query in
        ["after=0".to_owned(), "after=0&epoch=1".to_owned(), format!("after=1&epoch={epoch}")]
    {
        let (status, page) = json(&app, &format!("/api/v1/libraries/1/changes?{query}")).await;
        assert_eq!(
            (status, &page["epoch"]),
            (StatusCode::OK, &epoch.to_string().into()),
            "{query}"
        );
    }
}

#[tokio::test]
async fn each_library_counts_its_own_positions() {
    let (_temp, db, app) = catalog::app().await;
    record(&db, 1, &[(Entity::Track, 1001, Op::Upsert)]).await;
    let others: Vec<_> = (2001..2051).map(|id| (Entity::Track, id, Op::Upsert)).collect();
    record(&db, 2, &others).await;
    record(&db, 1, &[(Entity::Track, 1002, Op::Upsert), (Entity::Track, 1001, Op::Upsert)]).await;

    let (_, page) = json(&app, "/api/v1/libraries/1/changes").await;
    let seqs: Vec<_> = page["changes"].as_array().unwrap().iter().map(|c| &c["seq"]).collect();
    assert_eq!(seqs, [2, 3], "library 2's fifty changes leave no gap, and 1001 moved on");
    assert_eq!((&page["next"], &page["head"]), (&3.into(), &3.into()));
    let (_, theirs) = json(&app, "/api/v1/libraries/2/changes?limit=1").await;
    assert_eq!((&theirs["changes"][0]["seq"], &theirs["head"]), (&1.into(), &50.into()));

    // A device at library 1's position 2 is sent only what came after it there.
    let uri = format!("/api/v1/libraries/1/changes?after=2&epoch={}", db.epoch());
    let (_, later) = json(&app, &uri).await;
    let seqs: Vec<_> = later["changes"].as_array().unwrap().iter().map(|c| &c["seq"]).collect();
    assert_eq!(seqs, [3], "{later}");
}

#[tokio::test]
async fn a_restored_backup_refuses_a_position_it_never_reached_and_a_run_it_never_saw() {
    let (temp, db, app) = catalog::app().await;
    record(&db, 1, &[(Entity::Track, 1001, Op::Upsert), (Entity::Track, 1002, Op::Upsert)]).await;
    let backup = temp.path().join("backup.db");
    conn(&temp).execute("VACUUM INTO ?1", [backup.to_str()]).unwrap();
    // The run the backup was taken in goes on to 4; a later run, the backup never sees.
    record(&db, 1, &[(Entity::Album, 101, Op::Upsert), (Entity::Album, 102, Op::Upsert)]).await;
    let taken = db.epoch();
    drop((db, app));
    let later = crate::db::Database::open(&temp.path().join("jewelcase.db")).unwrap().epoch();

    let restored = tempfile::tempdir().unwrap();
    std::fs::copy(&backup, restored.path().join("jewelcase.db")).unwrap();
    let db = Arc::new(crate::db::Database::open(&restored.path().join("jewelcase.db")).unwrap());
    let app = router_over(state(&restored, db.clone()));
    // The restored server records more than the device had, so `after` is under its head.
    let more: Vec<_> = (1003..1008).map(|id| (Entity::Track, id, Op::Upsert)).collect();
    record(&db, 1, &more).await;

    let status = async |after: i64, epoch: i64| {
        let uri = format!("/api/v1/libraries/1/changes?after={after}&epoch={epoch}");
        json(&app, &uri).await.0
    };
    assert_eq!(status(4, taken).await, StatusCode::GONE, "past where the backup's run ended");
    assert_eq!(status(2, later).await, StatusCode::GONE, "a run after the backup");
    assert_eq!(status(2, taken).await, StatusCode::OK, "within the backup");
    assert_eq!(status(7, db.epoch()).await, StatusCode::OK, "the restored server's own run");

    // Restarted, the restored server still ends the backup's run where the copy did, not at the
    // head its own run reached.
    drop((db, app));
    let db = Arc::new(crate::db::Database::open(&restored.path().join("jewelcase.db")).unwrap());
    let app = router_over(state(&restored, db.clone()));
    let status = async |after: i64, epoch: i64| {
        let uri = format!("/api/v1/libraries/1/changes?after={after}&epoch={epoch}");
        json(&app, &uri).await.0
    };
    assert_eq!(status(4, taken).await, StatusCode::GONE, "past the backup, after a restart");
    assert_eq!(status(2, taken).await, StatusCode::OK, "within the backup, after a restart");
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
