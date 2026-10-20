//! Every signed-in route against the gates [`super::require`] applies: `401` without a valid
//! token, `403` under `/admin` for a user, and for a library the caller cannot reach the same
//! `404`, byte for byte, as for one that does not exist (`requirements/users.md` §10).

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use serde_json::{Value, json};

use super::super::testing::{TOKEN, add_device, add_user, app_with_tracks_at, respond};
use crate::db::testing::{Artist, Image, access};

/// Every route behind [`super::require`], as a method and a path under the API prefix with
/// sample IDs. Paths under a library name library 2 and something [`server`] puts in it, so a
/// handler reached past the gate would answer differently than for a library that does not exist.
/// `search`, `playback` and `audio` leave out their required query, so there a handler reached
/// past the gate answers `422`.
pub(super) const ROUTES: &[(&str, &str)] = &[
    ("POST", "/auth/logout"),
    ("GET", "/me"),
    ("GET", "/me/devices"),
    ("GET", "/me/player"),
    ("POST", "/me/player/commands"),
    ("POST", "/me/plays"),
    ("GET", "/me/plugins"),
    ("GET", "/me/plugins/lrclib-lyrics/settings"),
    ("PUT", "/me/plugins/lrclib-lyrics/settings"),
    ("DELETE", "/me/plugins/lrclib-lyrics/settings"),
    ("PUT", "/me/plugins/lrclib-lyrics/search-sharing"),
    ("GET", "/realtime"),
    ("GET", "/libraries"),
    ("GET", "/libraries/2"),
    ("GET", "/libraries/2/albums"),
    ("GET", "/libraries/2/albums/201"),
    ("GET", "/libraries/2/artists"),
    ("GET", "/libraries/2/artists/20"),
    ("GET", "/libraries/2/playlists"),
    ("GET", "/libraries/2/tracks"),
    ("GET", "/libraries/2/tracks/3"),
    ("GET", "/libraries/2/tracks/3/lyrics"),
    ("GET", "/libraries/2/tracks/3/waveform"),
    ("GET", "/libraries/2/tracks/3/playback"),
    ("GET", "/libraries/2/tracks/3/audio"),
    ("GET", "/libraries/2/search"),
    ("GET", "/libraries/2/recent-searches"),
    ("POST", "/libraries/2/recent-searches"),
    ("DELETE", "/libraries/2/recent-searches"),
    ("DELETE", "/libraries/2/recent-searches/20"),
    ("GET", "/libraries/2/images/20"),
    ("GET", "/admin/plugins"),
    ("POST", "/admin/plugins"),
    ("GET", "/admin/plugins/lrclib-lyrics"),
    ("DELETE", "/admin/plugins/lrclib-lyrics"),
    ("PUT", "/admin/plugins/lrclib-lyrics/permissions"),
    ("PUT", "/admin/plugins/lrclib-lyrics/exemptions/lrclib.net"),
    ("DELETE", "/admin/plugins/lrclib-lyrics/exemptions/lrclib.net"),
    ("PUT", "/admin/plugins/lrclib-lyrics/libraries/2"),
    ("POST", "/admin/plugins/lrclib-lyrics/run"),
    ("GET", "/admin/plugins/lrclib-lyrics/settings"),
    ("PUT", "/admin/plugins/lrclib-lyrics/settings"),
];

/// Each check runs at the root and under a base path, where the API's prefix is longer.
const BASES: [&str; 2] = ["", "/music"];

const USER: &str = "Bearer user-token";
const ADMIN: &str = "Bearer admin-token";

/// [`app_with_tracks_at`] `base`, with user 2 ([`USER`]) granted library 1 only, and admin 3
/// ([`ADMIN`]). Library 2 also holds artist 20, image 20, and recent search 20, which is user
/// 2's and just made, as if recorded before their access was taken away.
async fn server(base: &str) -> (tempfile::TempDir, Router) {
    let (temp, _db, app) = app_with_tracks_at(base).await;
    let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
    add_user(&conn, 2, "user");
    add_device(&conn, 2, 2, "user-token");
    access(&conn, 2, 1).unwrap();
    Artist::new(20, 2, "Elsewhere").insert(&conn).unwrap();
    Image::new(20, 2, "cover.png").insert(&conn).unwrap();
    conn.execute(
        "INSERT INTO recent_searches (id, user_id, library_id, query, folded, searched_at) \
         VALUES (20, 2, 2, 'a', 'a', unixepoch() * 1000)",
        [],
    )
    .unwrap();
    add_user(&conn, 3, "admin");
    add_device(&conn, 3, 3, "admin-token");
    (temp, app)
}

/// The status, headers, and body of `method` on `path` under `base`'s API, sent with `headers`
/// and no body.
async fn send(
    app: &Router,
    base: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut request = Request::builder().method(method).uri(format!("{base}/api/v1{path}"));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    respond(app.clone(), request.body(Body::empty()).unwrap()).await
}

#[tokio::test]
async fn every_route_is_unauthenticated_without_a_valid_token() {
    let cases: [&[(&str, &str)]; 6] = [
        &[],
        &[("authorization", "Bearer not-a-token")],
        &[("authorization", "Basic b3duZXI6cGFzcw==")],
        &[("authorization", "Bearer")],
        &[("cookie", "jewelcase_session=not-a-token")],
        &[("cookie", "jewelcase_session_old=owner-token")],
    ];
    for base in BASES {
        let (_temp, app) = server(base).await;
        for headers in cases {
            for &(method, path) in ROUTES {
                let (status, response, body) = send(&app, base, method, path, headers).await;
                let at = format!("{method} {base}{path} {headers:?}");
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{at}");
                let problem: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(problem["code"], "unauthenticated", "{at}");
                assert_eq!(response[header::WWW_AUTHENTICATE], "Bearer", "{at}");
            }
        }
    }
}

#[tokio::test]
async fn only_admins_pass_under_admin() {
    let user = [("authorization", USER)];
    for base in BASES {
        let (_temp, app) = server(base).await;
        for &(method, path) in ROUTES.iter().filter(|(_, path)| path.starts_with("/admin/")) {
            let at = format!("{method} {base}{path}");
            let (status, _, body) = send(&app, base, method, path, &user).await;
            let problem: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                (status, &problem["code"]),
                (StatusCode::FORBIDDEN, &json!("forbidden")),
                "{at}"
            );
            for admin in [ADMIN, &format!("Bearer {TOKEN}")] {
                let (status, _, _) =
                    send(&app, base, method, path, &[("authorization", admin)]).await;
                assert_ne!(status, StatusCode::FORBIDDEN, "{at} as {admin}");
            }
        }
    }
}

#[tokio::test]
async fn a_library_out_of_reach_answers_exactly_as_one_that_does_not_exist() {
    let user = [("authorization", USER)];
    let admin = [("authorization", ADMIN)];
    let owner_token = format!("Bearer {TOKEN}");
    let owner = [("authorization", owner_token.as_str())];
    for base in BASES {
        let (temp, app) = server(base).await;
        // Every sample ID a GET names is there: the owner, who reaches library 2, finds each one.
        // The GETs of `search`, `playback` and `audio` answer `422`, for their missing query.
        for &(method, path) in ROUTES
            .iter()
            .filter(|(method, path)| *method == "GET" && path.starts_with("/libraries/2"))
        {
            let (status, _, _) = send(&app, base, method, path, &owner).await;
            assert_ne!(status, StatusCode::NOT_FOUND, "{method} {base}{path} as the owner");
        }
        let under_library = ROUTES
            .iter()
            .filter_map(|&(method, path)| Some((method, path, path.strip_prefix("/libraries/2")?)));
        for (method, path, rest) in under_library {
            let at = format!("{method} {base}{path}");
            let unreached = send(&app, base, method, path, &user).await;
            let missing = send(&app, base, method, &format!("/libraries/9{rest}"), &user).await;
            assert_eq!(unreached.0, StatusCode::NOT_FOUND, "{at}");
            assert_eq!(unreached, missing, "{at}");
        }
        // The recent search the table deletes is there, and the user's attempt left it alone.
        let conn = rusqlite::Connection::open(temp.path().join("jewelcase.db")).unwrap();
        let kept: i64 = conn
            .query_row(
                "SELECT count(*) FROM recent_searches WHERE id = 20 AND user_id = 2 \
                 AND library_id = 2",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kept, 1);
        let get = |path, headers| send(&app, base, "GET", path, headers);
        assert_eq!(get("/libraries/x/tracks", &user).await.0, StatusCode::NOT_FOUND);
        assert_eq!(get("/libraries/1/tracks", &user).await.0, StatusCode::OK);
        assert_eq!(get("/libraries/2/tracks", &admin).await.0, StatusCode::OK);
        assert_eq!(get("/libraries/9", &admin).await.0, StatusCode::NOT_FOUND);

        // Revoking access takes effect on the next request.
        conn.execute("DELETE FROM library_access WHERE user_id = 2", []).unwrap();
        assert_eq!(get("/libraries/1/tracks", &user).await.0, StatusCode::NOT_FOUND);
    }
}

/// The router cannot list its routes to compare with [`ROUTES`], so this checks what it can:
/// every method the router serves on a path in [`ROUTES`] is in [`ROUTES`] too.
#[tokio::test]
async fn the_table_lists_every_method_on_its_paths() {
    let (_temp, app) = server("").await;
    for &(_, path) in ROUTES {
        for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
            if !ROUTES.contains(&(method, path)) {
                let (status, _, _) = send(&app, "", method, path, &[]).await;
                assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{method} {path}");
            }
        }
    }
}
