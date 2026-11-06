//! WebAssembly bindings for the device catalog, for the web client's catalog worker
//! (`design/general.md` §7.1): SQLite, as rusqlite builds it for the browser, on OPFS in a
//! dedicated worker or in memory (node, the unit tests). Natively this crate is empty.
#![cfg(target_family = "wasm")]

use jewelcase_catalog::{SCHEMA_VERSION, create_schema};
use rusqlite::{Connection, OpenFlags};
use sqlite_wasm_rs::WasmOsCallback;
use sqlite_wasm_vfs::sahpool::{OpfsSAHPoolCfg, install};
use wasm_bindgen::prelude::*;

mod bench;

/// The sahpool VFS's registered name, which `open` names when opening the file.
const VFS: &str = "opfs-sahpool";

/// The device database, held in WebAssembly memory and (for `open`) on OPFS. Call `free()` when
/// done with one.
#[wasm_bindgen]
pub struct Catalog(Connection);

#[wasm_bindgen]
impl Catalog {
    /// Opens the database file `name` on OPFS, creating it with the schema when it is new. Only
    /// a dedicated worker can, and one connection at a time may hold the file. Throws when OPFS
    /// is unavailable or the file belongs to another schema version, which the caller deletes.
    pub async fn open(name: String) -> Result<Catalog, JsError> {
        install::<WasmOsCallback>(&OpfsSAHPoolCfg::default(), false).await?;
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        Self::ready(Connection::open_with_flags_and_vfs(name, flags, VFS)?)
    }

    /// Opens an empty in-memory database with the schema, for node and tests.
    #[wasm_bindgen(js_name = openInMemory)]
    pub fn open_in_memory() -> Result<Catalog, JsError> {
        Self::ready(Connection::open_in_memory()?)
    }

    /// The schema version the database carries.
    #[wasm_bindgen(js_name = schemaVersion)]
    pub fn schema_version(&self) -> Result<i32, JsError> {
        Ok(self.0.pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    /// The names of the database's tables.
    pub fn tables(&self) -> Result<Vec<String>, JsError> {
        let mut statement =
            self.0.prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")?;
        Ok(statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?)
    }

    /// Creates the schema in a new database, and checks the version of an existing one.
    fn ready(conn: Connection) -> Result<Catalog, JsError> {
        let catalog = Catalog(conn);
        match catalog.schema_version()? {
            0 => create_schema(&catalog.0)?,
            SCHEMA_VERSION => {}
            other => return Err(JsError::new(&format!("catalog schema version {other}"))),
        }
        Ok(catalog)
    }
}
