//! The files the device-catalog scale gate loads (`web/e2e/catalog-scale.bench.ts`), made from
//! the generated library, on request.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;

use crate::db::Database;
use crate::db::store::SqliteStore;
use crate::db::testing::library;
use crate::db::testing::scale;

/// Writes library 1 of a generated library ([`scale::size`] tracks) as the device schema holds it
/// into `target/catalog-scale`: `device.db`, the device database file, and `json/<table>.jsonl`,
/// the same rows one JSON array per line, BLOBs in hex. Each table's device columns are copied
/// from the server's table of the same name. Run it with
/// `cargo test --release -p jewelcase-server write_device_scale_files -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark data"]
fn write_device_scale_files() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/catalog-scale");
    let (server, building, device) =
        (out.join("server.db"), out.join("building.db"), out.join("device.db"));
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(out.join("json")).unwrap();

    let db = Arc::new(Database::open(&server).unwrap());
    db.write_blocking(|tx| library(tx, 1, "Music", "/music")).unwrap();
    let store = SqliteStore::new(db.clone()).unwrap();
    scale::fill(&store, scale::records(Path::new("/music"), scale::size()));
    drop((store, db));

    jewelcase_catalog::create_schema(&Connection::open(&building).unwrap()).unwrap();
    let conn = Connection::open(&server).unwrap();
    conn.execute("ATTACH ? AS device", [building.to_str().unwrap()]).unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM device.sqlite_schema WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for table in &tables {
        let columns: Vec<(String, String)> = conn
            .prepare("SELECT name, type FROM pragma_table_info(?, 'device')")
            .unwrap()
            .query_map([table], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let names: Vec<_> = columns.iter().map(|(name, _)| name.as_str()).collect();
        let names = names.join(", ");
        let scope = if table == "tag_types" { "" } else { " WHERE library_id = 1" };
        conn.execute_batch(&format!(
            "INSERT INTO device.{table} ({names}) SELECT {names} FROM main.{table}{scope}"
        ))
        .unwrap();

        let json: Vec<_> = columns
            .iter()
            .map(|(name, kind)| if kind == "BLOB" { format!("hex({name})") } else { name.clone() })
            .collect();
        let mut file =
            BufWriter::new(File::create(out.join(format!("json/{table}.jsonl"))).unwrap());
        let mut rows = conn
            .prepare(&format!("SELECT json_array({}) FROM device.{table}", json.join(", ")))
            .unwrap();
        let mut rows = rows.query([]).unwrap();
        let mut count = 0;
        while let Some(row) = rows.next().unwrap() {
            writeln!(file, "{}", row.get::<_, String>(0).unwrap()).unwrap();
            count += 1;
        }
        file.flush().unwrap();
        println!("{table}: {count} rows");
    }
    conn.execute_batch("ANALYZE device").unwrap();
    // Rebuilds the file compactly, as a snapshot builder would, rather than as inserts left it.
    conn.execute("VACUUM device INTO ?", [device.to_str().unwrap()]).unwrap();
    println!("device.db: {} bytes", fs::metadata(&device).unwrap().len());
    drop(conn);
    for file in [&server, &building] {
        let _ = fs::remove_file(file);
    }
}
