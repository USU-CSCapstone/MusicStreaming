//! The store's share of the scanning budgets, run on request.

use std::path::Path;

use jewelcase_scanner::testing;
use jewelcase_scanner::*;

use super::{make, record};

/// The cold-scan budget, store side only (`design/scanning.md` §15,
/// `requirements/performance.md` §5): does persisting a large library and
/// reading what is indexed in every folder fit inside an hour? Run with
/// `cargo test --release -p jewelcase-server benchmark_cold_scan -- --ignored --nocapture`.
/// `BUDGET_TRACKS` sets the library size (default 50,000); each time is also printed scaled
/// to 500,000 tracks.
#[test]
#[ignore = "benchmark"]
fn benchmark_cold_scan_store_only() {
    use std::time::Instant;
    let tracks: usize =
        std::env::var("BUDGET_TRACKS").ok().and_then(|v| v.parse().ok()).unwrap_or(50_000);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let store = make(&root);
    let lib = testing::library(&root);

    let synth = |i: usize| record(&root, i);

    let started = Instant::now();
    let batch_size = 250;
    let mut i = 0;
    while i < tracks {
        let mut batch = Batch::default();
        for j in i..(i + batch_size).min(tracks) {
            batch.upserts.push(synth(j));
        }
        store.apply(lib.id, batch).unwrap();
        i += batch_size;
    }
    let insert = started.elapsed();

    // An unchanged rescan reads what is indexed in each folder, root and artists included, and
    // writes nothing.
    let started = Instant::now();
    let mut folders = vec![root.clone()];
    folders.extend(
        (0..tracks).step_by(10).filter_map(|i| synth(i).path.parent().map(Path::to_path_buf)),
    );
    folders.extend((0..tracks).step_by(100).map(|i| root.join(format!("Artist {}", i / 100))));
    let read: usize =
        folders.iter().map(|f| store.indexed_in(lib.id, f).unwrap().files.len()).sum();
    let reads = started.elapsed();
    assert_eq!(read, tracks);

    let per_500k = |d: std::time::Duration| d.as_secs_f64() * 500_000.0 / tracks as f64;
    eprintln!(
        "{tracks} tracks: insert {insert:.2?} ({:.0}s per 500k), unchanged rescan's {} folder reads {reads:.2?} ({:.1}s per 500k)",
        per_500k(insert),
        folders.len(),
        per_500k(reads)
    );
}
