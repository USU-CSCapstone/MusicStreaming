//! `api/openapi.yaml` and the router agree: the server serves every operation the spec lists,
//! except those marked planned, and no other method on a path the spec lists.

use axum::http::StatusCode;

use super::super::testing::{app_before_setup, send};

/// The spec, read at build time.
const SPEC: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../api/openapi.yaml"));

/// The methods the spec and the router use.
const METHODS: [&str; 5] = ["get", "put", "post", "patch", "delete"];

/// Each path in the spec with its operations, as each method and whether its description marks
/// it planned. Read from the spec's layout: a path at two spaces' indent under `paths:`, its
/// methods at four. The methods under a key it cannot read as a path are left out.
fn paths() -> Vec<(&'static str, Vec<(&'static str, bool)>)> {
    let mut paths: Vec<(&str, Vec<(&str, bool)>)> = Vec::new();
    let mut in_path = false;
    let lines = SPEC.lines().skip_while(|line| *line != "paths:").skip(1);
    for line in lines.take_while(|line| *line != "components:") {
        let key =
            line.strip_prefix("  ").filter(|key| !key.is_empty() && !key.starts_with([' ', '#']));
        let method = line.strip_prefix("    ").and_then(|key| key.strip_suffix(':'));
        if let Some(key) = key {
            let path = key.strip_prefix('/').and_then(|path| path.strip_suffix(':'));
            in_path = path.is_some();
            paths.extend(path.map(|path| (path, Vec::new())));
        } else if let Some((_, operations)) = paths.last_mut().filter(|_| in_path) {
            match method.filter(|method| METHODS.contains(method)) {
                Some(method) => operations.push((method, false)),
                None if line.contains("**Planned:**") => {
                    if let Some((_, planned)) = operations.last_mut() {
                        *planned = true;
                    }
                }
                None => {}
            }
        }
    }
    paths
}

/// Before setup, every route answers something other than the fallbacks' `404` and `405`.
#[tokio::test]
async fn the_router_serves_what_the_spec_lists_and_nothing_else_on_its_paths() {
    let (_temp, _db, app) = app_before_setup("");
    let paths = paths();
    // Every operation was read, so none on a path line the parser missed goes unchecked.
    let read: usize = paths.iter().map(|(_, operations)| operations.len()).sum();
    let ids = SPEC.lines().filter(|line| line.trim_start().starts_with("operationId:")).count();
    assert_eq!(read, ids, "operations read from the spec");
    for (path, operations) in paths {
        // Any value fills a parameter: setup's check answers before a handler reads it.
        let segments = path.split('/').map(|s| if s.starts_with('{') { "1" } else { s });
        let uri = format!("/api/v1/{}", segments.collect::<Vec<_>>().join("/"));
        for method in METHODS {
            let (status, _, _) = send(app.clone(), &method.to_uppercase(), &uri).await;
            let served = ![StatusCode::NOT_FOUND, StatusCode::METHOD_NOT_ALLOWED].contains(&status);
            let listed = operations.iter().find(|(listed, _)| *listed == method);
            let expected = listed.is_some_and(|(_, planned)| !planned);
            assert_eq!(served, expected, "{method} /{path} answers {status}");
        }
    }
}
