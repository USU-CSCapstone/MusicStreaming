//! A page connects only from this server's own pages.

use axum::http::StatusCode;

use super::status;
use crate::api::session::COOKIE;
use crate::api::testing::socket::{connect, serve};
use crate::api::testing::{TOKEN, app};

#[tokio::test]
async fn a_page_connects_only_from_this_server() {
    let (_temp, _db, app) = app("");
    let address = serve(app).await;
    let cookie = format!("{COOKIE}={TOKEN}");

    let here = format!("http://{address}");
    assert!(connect(address, &[("cookie", &cookie), ("origin", &here)]).await.is_ok());
    // Behind a proxy that changes `Host`, or names or leaves out a default port.
    for (origin, header, host) in [
        ("https://music.example", "x-forwarded-host", "music.example"),
        ("https://music.example", "host", "music.example:443"),
        ("https://music.example:443", "host", "music.example"),
        ("https://Music.Example:8443", "host", "music.example:8443"),
        ("http://[::1]:8443", "host", "[::1]:8443"),
    ] {
        let proxied = [("cookie", &*cookie), ("origin", origin), (header, host)];
        assert!(connect(address, &proxied).await.is_ok(), "{origin} {header}: {host}");
    }

    let other_port = format!("http://{}:1", address.ip());
    for origin in ["https://evil.example", "null", &other_port] {
        let refused =
            connect(address, &[("cookie", &cookie), ("origin", origin)]).await.unwrap_err();
        assert_eq!(status(refused), StatusCode::FORBIDDEN, "{origin}");
    }
    for (origin, host) in [
        ("https://music.example:9000", "music.example:8443"),
        ("https://music.example", "music.example:8443"),
        // Another server on the same host: the same site, so sent the cookie.
        ("https://music.example:8443", "music.example"),
        ("http://music.example:9999", "music.example"),
        ("http://music.example", "music.example:443"),
        ("https://evil.example:8443", "music.example:8443"),
        ("http://[::2]:8443", "[::1]:8443"),
    ] {
        let proxied = [("cookie", &*cookie), ("origin", origin), ("x-forwarded-host", host)];
        let refused = connect(address, &proxied).await.unwrap_err();
        assert_eq!(status(refused), StatusCode::FORBIDDEN, "{origin} {host}");
    }
}
