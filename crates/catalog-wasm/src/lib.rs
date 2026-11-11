//! WebAssembly bindings for the device catalog, for the web client's catalog worker
//! (`design/general.md` §7.1): SQLite, as rusqlite builds it for the browser, on OPFS in a
//! dedicated worker or in memory (node, the unit tests). Natively this crate is empty.
#![cfg(target_family = "wasm")]

use jewelcase_catalog::page::Totals;
use jewelcase_catalog::{SCHEMA_VERSION, create_schema};
use jewelcase_core::Index;
use rusqlite::{Connection, OpenFlags};
use sqlite_wasm_rs::WasmOsCallback;
use sqlite_wasm_vfs::sahpool::{OpfsSAHPoolCfg, install};
use wasm_bindgen::prelude::*;

mod feed;
mod mutate;
mod query;
mod store;

/// The sahpool VFS's registered name, which `open` names when opening the file.
const VFS: &str = "opfs-sahpool";

/// The device schema version this build reads and writes, which a snapshot must be of.
#[wasm_bindgen(js_name = schemaVersion)]
pub fn schema_version() -> i32 {
    SCHEMA_VERSION
}

/// The device database, held in WebAssembly memory and (for `open`) on OPFS, with the search
/// index built from it once `buildIndex` has run, and the counted totals of lists it has answered,
/// which every write to the file lets go of. A file holds one library. Call `free()` when done
/// with one. A snapshot is imported under another file name and opened as a new `Catalog`, so an
/// import needs no clearing.
#[wasm_bindgen]
pub struct Catalog {
    conn: Connection,
    /// The search index, once built.
    index: Option<Index>,
    /// The memo of counted totals, which every write to the file must clear.
    totals: Totals,
}

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
        Ok(self.conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
    }

    /// The names of the database's tables.
    pub fn tables(&self) -> Result<Vec<String>, JsError> {
        let mut statement = self
            .conn
            .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")?;
        Ok(statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?)
    }

    /// Runs `statements` as they are. Only the sync's tests call it, to damage a file as a failing
    /// disk would; the app never does.
    pub fn sql(&self, statements: &str) -> Result<(), JsError> {
        self.totals.clear();
        Ok(self.conn.execute_batch(statements)?)
    }

    /// The value of the connection setting `name` (a PRAGMA), as text. Only the tests call it, to
    /// read back what `ready` set.
    pub fn pragma(&self, name: &str) -> Result<String, JsError> {
        let value: rusqlite::types::Value =
            self.conn.query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))?;
        Ok(match value {
            rusqlite::types::Value::Text(text) => text,
            rusqlite::types::Value::Integer(number) => number.to_string(),
            other => format!("{other:?}"),
        })
    }

    /// Sets the connection up for a file that one worker alone has open, on a pool where every
    /// commit is a flush: the lock is kept (no per-transaction lock traffic), the rollback journal
    /// is kept and zeroed rather than deleted and recreated (up to `journal_size_limit`), the page
    /// cache and temporary tables live in memory (a pool slot is not spent on a temp file), and
    /// more statements stay prepared. `synchronous` stays FULL.
    fn tune(conn: &Connection) -> rusqlite::Result<()> {
        conn.set_prepared_statement_cache_capacity(64);
        conn.execute_batch(
            "PRAGMA locking_mode = EXCLUSIVE; \
             PRAGMA journal_mode = PERSIST; \
             PRAGMA journal_size_limit = 16777216; \
             PRAGMA cache_size = -32768; \
             PRAGMA temp_store = MEMORY;",
        )
    }

    /// Tunes the connection (see `tune`), creates the schema in a new database, and checks the
    /// version of an existing one. Either way the feed state tables exist afterwards. A newer
    /// version's error says "newer", so the sync keeps that file for the build that wrote it.
    fn ready(conn: Connection) -> Result<Catalog, JsError> {
        Self::tune(&conn)?;
        let catalog = Catalog { conn, index: None, totals: Totals::default() };
        match catalog.schema_version()? {
            0 => create_schema(&catalog.conn)?,
            SCHEMA_VERSION => {}
            other if other > SCHEMA_VERSION => {
                return Err(JsError::new(&format!("newer catalog schema version {other}")));
            }
            other => return Err(JsError::new(&format!("catalog schema version {other}"))),
        }
        catalog.conn.execute_batch(feed::FEED_STATE)?;
        catalog.conn.execute_batch(mutate::UNDO_STATE)?;
        Ok(catalog)
    }
}
