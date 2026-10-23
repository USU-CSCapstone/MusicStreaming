//! The session cookie, sent again whenever a request refreshes its device's `last_seen_at`, so a
//! web session in use never reaches the cookie's `Max-Age` (`requirements/users.md` §4).

use axum::http::{HeaderMap, StatusCode, header};

use super::super::super::testing::{TOKEN, app, conn};
use super::gates::send;

/// Every value of the header `name` in `headers`.
fn all(headers: &HeaderMap, name: header::HeaderName) -> Vec<&str> {
    headers.get_all(name).iter().map(|value| value.to_str().unwrap()).collect()
}

#[tokio::test]
async fn a_cookie_request_from_a_device_not_seen_lately_gets_its_cookie_again() {
    let cookie = format!("jewelcase_session={TOKEN}");
    let headers = [("cookie", cookie.as_str())];
    for base in ["", "/music"] {
        let (temp, _db, app) = app(base);
        // The device was last seen at the epoch, so this request refreshes it.
        let (status, response, _) = send(&app, base, "GET", "/me", &headers).await;
        assert_eq!(status, StatusCode::OK);
        let refreshed = format!(
            "jewelcase_session={TOKEN}; Path={base}/api/v1; Max-Age=34560000; HttpOnly; \
             SameSite=Strict"
        );
        assert_eq!(all(&response, header::SET_COOKIE), [refreshed], "{base}");
        // No shared cache may keep the token.
        assert_eq!(all(&response, header::CACHE_CONTROL), ["private"], "{base}");

        // Seen within the last few minutes: neither refreshed nor sent again.
        let conn = conn(&temp);
        conn.execute("UPDATE devices SET last_seen_at = unixepoch() * 1000", []).unwrap();
        let (_, response, _) = send(&app, base, "GET", "/me", &headers).await;
        assert!(all(&response, header::SET_COOKIE).is_empty(), "{base}");
    }
}

#[tokio::test]
async fn a_bearer_request_gets_no_cookie() {
    let (_temp, _db, app) = app("");
    let bearer = format!("Bearer {TOKEN}");
    let (status, response, _) = send(&app, "", "GET", "/me", &[("authorization", &bearer)]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(all(&response, header::SET_COOKIE).is_empty());
}

#[tokio::test]
async fn logging_out_by_cookie_from_a_device_not_seen_lately_only_forgets_it() {
    let (_temp, _db, app) = app("");
    let cookie = format!("jewelcase_session={TOKEN}");
    let headers = [("cookie", cookie.as_str()), ("x-jewelcase-client", "web")];
    let (status, response, _) = send(&app, "", "POST", "/auth/logout", &headers).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(all(&response, header::SET_COOKIE), ["jewelcase_session=; Path=/api/v1; Max-Age=0"]);
}
