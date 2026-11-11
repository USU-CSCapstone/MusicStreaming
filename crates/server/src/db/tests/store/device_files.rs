//! The file the device-catalog scale gate loads (`web/e2e/catalog-scale.bench.ts`), made from the
//! generated library, on request.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;

use crate::db::Database;
use crate::db::store::SqliteStore;
use crate::db::testing::library;
use crate::db::testing::scale;

/// Writes library 1 of a generated library ([`scale::size`] tracks) as the device schema holds it
/// into `target/catalog-scale/device.db`, copied as a catalog snapshot is
/// ([`jewelcase_catalog::snapshot::copy`]). Run it with
/// `cargo test --release -p jewelcase-server write_device_scale_files -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark data"]
fn write_device_scale_files() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/catalog-scale");
    let (server, building, device) =
        (out.join("server.db"), out.join("building.db"), out.join("device.db"));
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();

    let db = Arc::new(Database::open(&server).unwrap());
    db.write_blocking(|tx| library(tx, 1, "Music", "/music")).unwrap();
    let store = SqliteStore::new(db.clone()).unwrap();
    scale::fill(&store, scale::records(Path::new("/music"), scale::size()));
    drop((store, db));

    let mut conn = Connection::open(&building).unwrap();
    jewelcase_catalog::create_schema(&conn).unwrap();
    conn.execute("ATTACH ? AS server", [server.to_str().unwrap()]).unwrap();
    jewelcase_catalog::snapshot::copy(&mut conn, 1).unwrap();
    conn.execute_batch("DETACH server; ANALYZE").unwrap();
    // Rebuilds the file compactly, as the snapshot build does, rather than as inserts left it.
    conn.execute("VACUUM INTO ?", [device.to_str().unwrap()]).unwrap();
    println!("device.db: {} bytes", fs::metadata(&device).unwrap().len());
    drop(conn);
    for file in [&server, &building] {
        let _ = fs::remove_file(file);
    }
}
