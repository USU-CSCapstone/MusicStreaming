//! Bench-only entry points for the device-catalog scale gate (`web/e2e/catalog-scale.bench.ts`):
//! the streaming import of a database file onto OPFS, a chunked apply of JSON rows, and raw SQL.
//! Temporary: the real import and apply replace this module.

use std::cell::Cell;

use rusqlite::params_from_iter;
use sqlite_wasm_rs::WasmOsCallback;
use sqlite_wasm_rs::vfs::transfer::{DbImport, DbTransfer};
use sqlite_wasm_vfs::sahpool::{OpfsSAHImportTarget, OpfsSAHPoolCfg, OpfsSAHPoolUtil, install};
use wasm_bindgen::prelude::*;

use super::Catalog;

thread_local! {
    static POOL: Cell<Option<&'static OpfsSAHPoolUtil>> = const { Cell::new(None) };
}

/// The worker's sahpool handle, installed on first use and kept for the worker's life, since an
/// import borrows it.
async fn pool() -> Result<&'static OpfsSAHPoolUtil, JsError> {
    if let Some(pool) = POOL.get() {
        return Ok(pool);
    }
    let util = install::<WasmOsCallback>(&OpfsSAHPoolCfg::default(), false).await?;
    let pool: &'static OpfsSAHPoolUtil = Box::leak(Box::new(util));
    POOL.set(Some(pool));
    Ok(pool)
}

/// A database file being streamed onto OPFS, which `finish` publishes and dropping aborts.
#[wasm_bindgen]
pub struct Import(DbImport<OpfsSAHImportTarget<'static>>);

#[wasm_bindgen]
impl Import {
    /// Starts importing a database file of `size` bytes (under 4 GiB) as `name`. Nothing is visible under
    /// `name` until `finish`.
    pub async fn begin(name: String, size: u32) -> Result<Import, JsError> {
        Ok(Import(pool().await?.begin_import(&name, u64::from(size))?))
    }

    /// Appends the file's next bytes.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), JsError> {
        Ok(self.0.write(bytes)?)
    }

    /// Publishes the file once every byte has been written.
    pub fn finish(self) -> Result<(), JsError> {
        Ok(self.0.finish()?)
    }
}

#[wasm_bindgen]
impl Catalog {
    /// The database's `PRAGMA quick_check` result: `ok`, or its first complaint.
    #[wasm_bindgen(js_name = quickCheck)]
    pub fn quick_check(&self) -> Result<String, JsError> {
        Ok(self.conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?)
    }

    /// The first column of the first row `sql` selects as text, or an empty string when it
    /// selects none, with `params` bound as text.
    pub fn sql(&self, sql: &str, params: Vec<String>) -> Result<String, JsError> {
        let mut statement = self.conn.prepare_cached(sql)?;
        let mut rows = statement.query(params_from_iter(params))?;
        Ok(rows.next()?.map(|row| row.get(0)).transpose()?.unwrap_or_default())
    }

    /// The first column of every row `sql` selects, each an integer.
    pub fn ids(&self, sql: &str) -> Result<Vec<i64>, JsError> {
        let mut statement = self.conn.prepare_cached(sql)?;
        Ok(statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?)
    }

    /// Inserts `rows` (a JSON array of arrays, one value per column of `table` in table order,
    /// BLOBs as hex) in one transaction.
    #[wasm_bindgen(js_name = applyRows)]
    pub fn apply_rows(&self, table: &str, rows: &str) -> Result<(), JsError> {
        let mut statement = self.conn.prepare("SELECT name, type FROM pragma_table_info(?)")?;
        let columns: Vec<(String, String)> = statement
            .query_map([table], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let names = columns.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", ");
        let values = columns
            .iter()
            .enumerate()
            .map(|(i, (_, kind))| match kind.as_str() {
                "BLOB" => format!("unhex(j.value ->> {i})"),
                _ => format!("j.value ->> {i}"),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            &format!("INSERT INTO {table} ({names}) SELECT {values} FROM json_each(?) AS j"),
            [rows],
        )?;
        Ok(tx.commit()?)
    }
}
