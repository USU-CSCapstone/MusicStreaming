use super::super::http::{Error, Host as _};
use super::{body, get, run, server};

#[tokio::test]
async fn follows_redirects_only_to_approved_destinations() {
    let (port, _) = server().await;
    let mut run = run();
    let reply = run.send(get(format!("http://127.0.0.1:{port}/near"))).await.unwrap();
    assert_eq!((reply.status, reply.body.as_slice()), (200, b"hello".as_slice()));
    assert!(reply.headers.iter().any(|(name, value)| name == "x-served" && value == "yes"));

    let failed = run.send(get(format!("http://127.0.0.1:{port}/away"))).await.unwrap_err();
    assert!(
        matches!(&failed, Error::Failed(why) if why.contains("redirected elsewhere: localhost is not one of")),
        "{failed:?}"
    );
}

#[tokio::test]
async fn the_host_header_is_the_hosts() {
    let (port, _) = server().await;
    let mut request = get(format!("http://127.0.0.1:{port}/"));
    request.headers.push(("Host".into(), "elsewhere.example".into()));
    let refused = run().send(request).await.unwrap_err();
    assert!(
        matches!(&refused, Error::Refused(why) if why == "Host is set by the host"),
        "{refused:?}"
    );
}

/// Each refusal is recorded once, by host or permission; a string that is not a URL, or a
/// scheme other than http(s), names neither.
#[tokio::test]
async fn needs_the_network_and_an_approved_destination() {
    let mut run = run();
    for _ in 0..2 {
        let refused = run.send(get("http://example.com/".into())).await.unwrap_err();
        assert!(
            matches!(&refused, Error::Refused(why) if why.contains("not one of")),
            "{refused:?}"
        );
    }
    let refused = run.send(get("not a url".into())).await.unwrap_err();
    assert!(matches!(&refused, Error::Refused(why) if why == "not a URL"), "{refused:?}");
    let refused = run.send(get("ftp://127.0.0.1/".into())).await.unwrap_err();
    assert!(matches!(&refused, Error::Refused(why) if why.starts_with("ftp")), "{refused:?}");
    run.grants.permissions.clear();
    for _ in 0..2 {
        let refused = run.send(get("http://127.0.0.1/".into())).await.unwrap_err();
        assert!(matches!(refused, Error::NotGranted), "{refused:?}");
    }
    assert_eq!(run.refused.named, ["example.com", "Network access"]);
    assert_eq!(run.refused.more, 0);
}

#[tokio::test]
async fn a_chain_of_six_redirects_is_refused() {
    let (port, hits) = server().await;
    let mut run = run();
    assert_eq!(body(&mut run, port, "/hops/5").await, "hello");
    let failed = run.send(get(format!("http://127.0.0.1:{port}/hops/6"))).await.unwrap_err();
    assert!(matches!(failed, Error::Failed(_)), "{failed:?}");
    assert_eq!(hits.lock().unwrap()["/hops/0"], 1, "the end of the chain is never asked");
}

#[tokio::test]
async fn the_headers_that_frame_a_request_are_the_hosts() {
    let reserved =
        ["Host", "Content-Length", "TRANSFER-ENCODING", "connection", "Upgrade", "TE", "trailer"];
    for name in reserved {
        let mut request = get("http://127.0.0.1/".into());
        request.headers.push((name.into(), "1".into()));
        let refused = run().send(request).await.unwrap_err();
        assert!(matches!(refused, Error::Refused(_)), "{name}: {refused:?}");
    }
}

#[tokio::test]
async fn a_request_body_is_at_most_16_mb() {
    let mut request = get("http://127.0.0.1/".into());
    request.body = vec![0; (16 << 20) + 1];
    let refused = run().send(request).await.unwrap_err();
    assert!(matches!(refused, Error::Refused(_)), "{refused:?}");
    // At the limit it passes the size check, and is refused for the header after it, so it is
    // never sent.
    let mut request = get("http://127.0.0.1/".into());
    request.body = vec![0; 16 << 20];
    request.headers.push(("Host".into(), "1".into()));
    let refused = run().send(request).await.unwrap_err();
    assert!(matches!(&refused, Error::Refused(why) if why.starts_with("Host")), "{refused:?}");
}
