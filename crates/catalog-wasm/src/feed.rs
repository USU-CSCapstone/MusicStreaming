//! Writing the device database from the feeds: changes staged a page at a time and applied at the
//! head a chunk at a time (`design/general.md` §3), with the position each feed has reached and
//! a mark while an apply is part-way. The staging, the positions and the marks are tables of the
//! database file but not of the device schema, which a downloaded snapshot replaces whole.

use std::collections::BTreeMap;

use jewelcase_catalog::sync::{self, Kind, Row, Rows};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use super::Catalog;

/// What a database file keeps besides the device schema: each feed's last applied position, the
/// changes pulled past it that are not applied yet, and the position an apply that is part-way
/// goes to.
pub(crate) const FEED_STATE: &str = "\
    CREATE TABLE IF NOT EXISTS feed_cursors (feed TEXT PRIMARY KEY, seq INTEGER NOT NULL) \
        WITHOUT ROWID; \
    CREATE TABLE IF NOT EXISTS staged_changes ( \
        feed TEXT NOT NULL, seq INTEGER NOT NULL, change TEXT NOT NULL, PRIMARY KEY (feed, seq) \
    ) WITHOUT ROWID; \
    CREATE TABLE IF NOT EXISTS feed_applying (feed TEXT PRIMARY KEY, through INTEGER NOT NULL) \
        WITHOUT ROWID;";

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

    /// Applies the oldest `limit` changes kept for `feed` in one transaction that also deletes
    /// them, and says whether none is left. Until the last is applied, the file marks `feed` as
    /// applying through `through` (see `applying`); the transaction that applies the last moves
    /// the feed's position to `through` and clears the mark. Between two calls a reader sees part
    /// of the changes, so the caller keeps readers off the file until this says it is done.
    /// Throws on a change that does not fit the schema, which leaves that chunk as it was.
    pub fn commit(&self, feed: &str, through: i64, limit: u32) -> Result<bool, JsError> {
        let tx = self.0.unchecked_transaction()?;
        let mut last = None;
        {
            let mut select = tx.prepare_cached(
                "SELECT seq, change FROM staged_changes WHERE feed = ?1 ORDER BY seq LIMIT ?2",
            )?;
            let mut staged = select.query(params![feed, limit])?;
            while let Some(row) = staged.next()? {
                last = Some(row.get::<_, i64>(0)?);
                let text = row.get_ref(1)?.as_str().map_err(rusqlite::Error::from)?;
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
        if let Some(last) = last {
            tx.execute(
                "DELETE FROM staged_changes WHERE feed = ?1 AND seq <= ?2",
                params![feed, last],
            )?;
        }
        let done = !tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM staged_changes WHERE feed = ?1)",
            [feed],
            |row| row.get::<_, bool>(0),
        )?;
        if done {
            tx.execute("DELETE FROM feed_applying WHERE feed = ?1", [feed])?;
            tx.execute(
                "INSERT OR REPLACE INTO feed_cursors (feed, seq) VALUES (?1, ?2)",
                params![feed, through],
            )?;
        } else {
            tx.execute(
                "INSERT OR REPLACE INTO feed_applying (feed, through) VALUES (?1, ?2)",
                params![feed, through],
            )?;
        }
        tx.commit()?;
        Ok(done)
    }

    /// The position an apply of `feed` that stopped part-way, as when its worker was closed, was
    /// going to: `commit` up to it finishes the apply. Undefined when none is part-way.
    pub fn applying(&self, feed: &str) -> Result<Option<i64>, JsError> {
        Ok(self
            .0
            .query_row("SELECT through FROM feed_applying WHERE feed = ?1", [feed], |row| {
                row.get(0)
            })
            .optional()?)
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
