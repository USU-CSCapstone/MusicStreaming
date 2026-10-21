//! Each reference plugin under `plugins/` builds against its own copy of the contract,
//! `wit/plugin.wit`, so that it builds outside this repository. These copies must stay the same
//! as the host's, `crates/plugins/wit/plugin.wit`.

use std::fs;
use std::path::Path;

/// Every folder under `plugins/` with a `Cargo.toml` is a reference plugin: its
/// `wit/plugin.wit` is byte for byte the host's, and its `src/lib.rs` generates its bindings
/// from that copy, with `generate!()`'s default path, rather than from the host's.
#[test]
fn every_reference_plugin_has_the_hosts_contract() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let contract = fs::read(crate_dir.join("wit/plugin.wit")).unwrap();
    let mut checked = Vec::new();
    for entry in fs::read_dir(crate_dir.join("../../plugins")).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.join("Cargo.toml").is_file() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let copy = fs::read(dir.join("wit/plugin.wit"))
            .unwrap_or_else(|e| panic!("plugins/{name}/wit/plugin.wit: {e}"));
        assert!(
            copy == contract,
            "plugins/{name}/wit/plugin.wit differs from crates/plugins/wit/plugin.wit: copy it again"
        );
        let lib = fs::read_to_string(dir.join("src/lib.rs")).unwrap();
        assert!(
            lib.contains("wit_bindgen::generate!();"),
            "plugins/{name}/src/lib.rs must build against its own copy, with generate!()"
        );
        checked.push(name);
    }
    checked.sort();
    assert_eq!(checked, ["listenbrainz", "lrclib-lyrics"], "the plugins found");
}
