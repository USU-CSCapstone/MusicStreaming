//! Bench-only entry points for the device-catalog scale gate (`web/e2e/catalog-scale.bench.ts`):
//! `quick_check`, a chunked apply of JSON rows, and raw SQL. Temporary: the real apply replaces
//! this module.

use rusqlite::params_from_iter;
use wasm_bindgen::prelude::*;

use super::Catalog;

#[wasm_bindgen]
impl Catalog {
    /// The database's `PRAGMA quick_check` result: `ok`, or its first complaint.
    #[wasm_bindgen(js_name = quickCheck)]
    pub fn quick_check(&self) -> Result<String, JsError> {
        Ok(self.0.query_row("PRAGMA quick_check", [], |row| row.get(0))?)
    }

    /// The first column of the first row `sql` selects as text, or an empty string when it
    /// selects none, with `params` bound as text.
    pub fn sql(&self, sql: &str, params: Vec<String>) -> Result<String, JsError> {
        let mut statement = self.0.prepare_cached(sql)?;
        let mut rows = statement.query(params_from_iter(params))?;
        Ok(rows.next()?.map(|row| row.get(0)).transpose()?.unwrap_or_default())
    }

    /// The first column of every row `sql` selects, each an integer.
    pub fn ids(&self, sql: &str) -> Result<Vec<i64>, JsError> {
        let mut statement = self.0.prepare_cached(sql)?;
        Ok(statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?)
    }

    /// Inserts `rows` (a JSON array of arrays, one value per column of `table` in table order,
    /// BLOBs as hex) in one transaction.
    #[wasm_bindgen(js_name = applyRows)]
    pub fn apply_rows(&self, table: &str, rows: &str) -> Result<(), JsError> {
        let mut statement = self.0.prepare("SELECT name, type FROM pragma_table_info(?)")?;
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
        let tx = self.0.unchecked_transaction()?;
        tx.execute(
            &format!("INSERT INTO {table} ({names}) SELECT {values} FROM json_each(?) AS j"),
            [rows],
        )?;
        Ok(tx.commit()?)
    }
}
