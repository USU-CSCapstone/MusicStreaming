//! Writing the device database from the feeds: rows applied at once, or changes staged a page at a
//! time and applied in one transaction at the head (`design/general.md` §3), with the position
//! each feed has reached. The staging and the positions are tables of the database file but not
//! of the device schema, which a downloaded snapshot replaces whole.

use std::collections::BTreeMap;

use jewelcase_catalog::sync::{self, Kind, Row, Rows};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use super::Catalog;

/// What a database file keeps besides the device schema: each feed's last applied position, and
/// the changes pulled past it that are not applied yet.
pub(crate) const FEED_STATE: &str = "\
    CREATE TABLE IF NOT EXISTS feed_cursors (feed TEXT PRIMARY KEY, seq INTEGER NOT NULL) \
        WITHOUT ROWID; \
    CREATE TABLE IF NOT EXISTS staged_changes ( \
        feed TEXT NOT NULL, seq INTEGER NOT NULL, change TEXT NOT NULL, PRIMARY KEY (feed, seq) \
    ) WITHOUT ROWID;";

/// A feed's change as the library and account feeds send it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Change {
    seq: i64,
    op: Op,
    kind: Kind,
    id: String,
    /// A playlist item's key; the library feed has none.
    #[serde(default)]
    key: i64,
    /// An upsert's rows by table.
    rows: Option<BTreeMap<String, Vec<Row>>>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Op {
    Upsert,
    Delete,
}

#[wasm_bindgen]
impl Catalog {
    /// Applies `rows` (a JSON array of an entity's rows, as the feed carries them) in one
    /// transaction, so a failure leaves none of them applied.
    pub fn apply(&self, rows: &str) -> Result<(), JsError> {
        let rows: Vec<Rows> = serde_json::from_str(rows)?;
        let tx = self.0.unchecked_transaction()?;
        for rows in &rows {
            sync::apply(&tx, rows)?;
        }
        Ok(tx.commit()?)
    }

    /// Keeps `changes` (a JSON array of a feed's changes, such as a page of the feed's response)
    /// for `feed` to apply later, replacing any already kept at the same positions. Nothing a
    /// reader sees changes until `commit`.
    pub fn stage(&self, feed: &str, changes: &str) -> Result<(), JsError> {
        let changes: Vec<Change> = serde_json::from_str(changes)?;
        let tx = self.0.unchecked_transaction()?;
        for change in &changes {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO staged_changes (feed, seq, change) VALUES (?1, ?2, ?3)",
            )?
            .execute(params![feed, change.seq, serde_json::to_string(change)?])?;
        }
        Ok(tx.commit()?)
    }

    /// Applies every change kept for `feed` in order, and moves its position to `through` (the
    /// feed's head, or its next position), all in one transaction: a reader sees the database
    /// before or after, never between, and a failure changes nothing. Throws on a change that
    /// does not fit the schema.
    pub fn commit(&self, feed: &str, through: i64) -> Result<(), JsError> {
        let tx = self.0.unchecked_transaction()?;
        {
            let mut select =
                tx.prepare("SELECT change FROM staged_changes WHERE feed = ?1 ORDER BY seq")?;
            let mut staged = select.query([feed])?;
            while let Some(row) = staged.next()? {
                let text = row.get_ref(0)?.as_str().map_err(rusqlite::Error::from)?;
                let Change { op, kind, id, key, rows, .. } = serde_json::from_str(text)?;
                match (op, rows) {
                    (Op::Upsert, Some(tables)) => {
                        sync::apply(&tx, &Rows { kind, id, key, tables })?;
                    }
                    (Op::Upsert, None) => {
                        return Err(JsError::new("a staged upsert carries no rows"));
                    }
                    (Op::Delete, _) => {
                        let id =
                            id.parse().map_err(|_| JsError::new("a staged ID is not an ID"))?;
                        sync::delete(&tx, kind, id, key)?;
                    }
                }
            }
        }
        tx.execute("DELETE FROM staged_changes WHERE feed = ?1", [feed])?;
        tx.execute(
            "INSERT OR REPLACE INTO feed_cursors (feed, seq) VALUES (?1, ?2)",
            params![feed, through],
        )?;
        Ok(tx.commit()?)
    }

    /// The position `feed` was last applied through, or 0 if it never was.
    pub fn cursor(&self, feed: &str) -> Result<i64, JsError> {
        let seq = self
            .0
            .query_row("SELECT seq FROM feed_cursors WHERE feed = ?1", [feed], |row| row.get(0))
            .optional()?;
        Ok(seq.unwrap_or(0))
    }

    /// The position a pull of `feed` resumes after: its last staged change, or its cursor when
    /// none is staged.
    pub fn pulled(&self, feed: &str) -> Result<i64, JsError> {
        let staged: Option<i64> = self.0.query_row(
            "SELECT max(seq) FROM staged_changes WHERE feed = ?1",
            [feed],
            |row| row.get(0),
        )?;
        match staged {
            Some(seq) => Ok(seq),
            None => self.cursor(feed),
        }
    }
}
