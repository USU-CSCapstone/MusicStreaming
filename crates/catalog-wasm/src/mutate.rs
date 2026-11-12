//! The account's writes on the device database, through the code the server's handlers share
//! ([`jewelcase_catalog::mutate`]), each kept with its undo in the file it changed until the
//! device rewinds them (`design/web.md`, The sync framework). A write and its undo are committed in
//! one transaction, and a rewind reverts and forgets them in another, so a file never holds a
//! write it cannot take back, nor an undo of a write it does not hold. A write the server answered
//! is kept, rewound, to be made again, until a rewind for the account feed drops it.

use std::collections::BTreeSet;

use jewelcase_catalog::liked;
use jewelcase_catalog::mutate::{self, Effect, Undo};
use jewelcase_catalog::query::number;
use rusqlite::{ErrorCode, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use super::Catalog;

/// The writes applied to the file, in the order they were applied, each under the ID of the call
/// that made it, with what made it (`name`, `args`, `at`) and, while it is applied, its undo. One
/// the server `answered` stays once rewound, with no undo (`''`), to be made again. Not part of the
/// device schema: a snapshot starts without it, as it starts without any of the device's own
/// writes.
pub(crate) const UNDO_STATE: &str = "\
    CREATE TABLE IF NOT EXISTS mutation_undo ( \
        seq INTEGER PRIMARY KEY, call TEXT NOT NULL UNIQUE, undo TEXT NOT NULL, \
        name TEXT NOT NULL, args TEXT NOT NULL, at INTEGER NOT NULL, answered INTEGER NOT NULL \
    );";

/// One write as the page names it (`Call` in `web/src/lib/sync/mutation.ts`).
#[derive(Serialize, Deserialize)]
struct Call {
    id: String,
    name: String,
    args: serde_json::Value,
    at: f64,
}

/// The kinds of rows a list depends on (`web/src/lib/catalog/kinds.ts`) that `effect` changed, given
/// the user's Liked Songs, `liked`.
fn kinds(effect: Effect, liked: Option<i64>) -> &'static [&'static str] {
    match effect {
        Effect::Playlist { id } | Effect::Item { playlist: id, .. } if Some(id) == liked => {
            &["playlists", "liked"]
        }
        Effect::Playlist { .. } | Effect::Item { .. } => &["playlists"],
        Effect::Hidden { .. } => &["hidden"],
        Effect::Liked { .. } => &["liked"],
        Effect::Search { .. } => &["searches"],
    }
}

/// Whether a failed write's database error is that one write's, as a constraint it breaks, rather
/// than the file's.
fn its_own(error: &rusqlite::Error) -> bool {
    error.sqlite_error_code() == Some(ErrorCode::ConstraintViolation)
}

#[wasm_bindgen]
impl Catalog {
    /// Applies `calls`, JSON text of `[{id, name, args, at}]` in order, each the write `name`
    /// (`like`, `unlike`, `hide`, `unhide`, `createPlaylist`, `editPlaylist`, `appendToPlaylist`,
    /// `deletePlaylist`, `recordSearch`, `deleteSearch` or `clearSearches`) with `args` spelled as
    /// `jewelcase_catalog::mutate::apply` describes, for `user` in `library` (decimal IDs), at
    /// `at`, milliseconds since the epoch, which the caller keeps with the write so that applying
    /// it again makes the same rows. `id` names the call: one the file holds already is not
    /// applied again. All of them are one transaction, and each call a savepoint in it, so a
    /// refusal (as `jewelcase_catalog::mutate::Error::refusal` names them) or a constraint the
    /// call breaks changes nothing of that call alone. With `answered`, the server has the calls,
    /// and nothing is applied: each the file holds is kept to be made again after a rewind that
    /// does not drop it, and one it lacks is left to the feed, which may have brought it already.
    /// Answers JSON text:
    /// `{"made", "kinds", "failed"}`, how many were applied, the kinds of rows they changed, and
    /// the messages of the constraints broken. Any other database failure throws, with nothing
    /// changed.
    #[wasm_bindgen(js_name = applyCalls)]
    pub fn apply_calls(
        &self,
        user: &str,
        library: &str,
        calls: &str,
        answered: bool,
    ) -> Result<String, JsError> {
        self.totals.clear();
        let (user, library) = (number(user, "user")?, number(library, "library")?);
        let calls: Vec<Call> = serde_json::from_str(calls)?;
        let liked = liked::find(&self.conn, user, library)?.map(|(id, _)| id);
        let (mut made, mut failed) = (0, Vec::new());
        let mut changed: BTreeSet<&str> = BTreeSet::new();
        let mut tx = self.conn.unchecked_transaction()?;
        for Call { id, name, args, at } in &calls {
            if answered {
                tx.prepare_cached(
                    "UPDATE mutation_undo SET answered = 1 WHERE call = ?1 AND undo <> ''",
                )?
                .execute([id])?;
                continue;
            }
            let held: Option<bool> = tx
                .prepare_cached("SELECT undo <> '' FROM mutation_undo WHERE call = ?1")?
                .query_row([id], |row| row.get(0))
                .optional()?;
            if held == Some(true) {
                continue;
            }
            let args = args.to_string();
            // Dropped unreleased after a failure, which rolls back what the call wrote.
            let point = tx.savepoint()?;
            let mut effects = Vec::new();
            let applied =
                mutate::apply_in(&point, user, library, name, &args, *at as i64, &mut |e| {
                    effects.push(e);
                    Ok(())
                });
            match applied {
                Ok(undo) => {
                    point
                        .prepare_cached(
                            "INSERT INTO mutation_undo (call, undo, name, args, at, answered) \
                             VALUES (?1, ?2, ?3, ?4, ?5, 0) ON CONFLICT (call) DO UPDATE SET \
                             undo = excluded.undo",
                        )?
                        .execute(params![
                            id,
                            serde_json::to_string(&undo)?,
                            name,
                            args,
                            *at as i64
                        ])?;
                    point.commit()?;
                    made += 1;
                    changed.extend(effects.into_iter().flat_map(|e| kinds(e, liked)));
                }
                Err(error) if error.refusal().is_some() => {}
                Err(mutate::Error::Sql(error)) if its_own(&error) => failed.push(error.to_string()),
                Err(error) => return Err(error.into()),
            }
        }
        tx.commit()?;
        Ok(serde_json::json!({ "made": made, "kinds": changed, "failed": failed }).to_string())
    }

    /// Takes back the newest `limit` writes `applyCalls` applied that are applied still, newest
    /// first, putting their rows back exactly as they were, in one transaction. Each is forgotten,
    /// unless the server answered it and `drop` is false: that one is kept unapplied, for
    /// `answeredCalls`. With `drop`, once none is applied, every write kept unapplied is forgotten
    /// too. True once none is applied.
    pub fn rewind(&self, limit: u32, drop: bool) -> Result<bool, JsError> {
        self.totals.clear();
        let tx = self.conn.unchecked_transaction()?;
        let newest: Vec<(i64, String, bool)> = tx
            .prepare_cached(
                "SELECT seq, undo, answered FROM mutation_undo WHERE undo <> '' \
                 ORDER BY seq DESC LIMIT ?1",
            )?
            .query_map([limit], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (seq, undo, answered) in &newest {
            let undo: Undo = serde_json::from_str(undo)?;
            mutate::revert_in(&tx, &undo)?;
            let forget = if *answered && !drop {
                "UPDATE mutation_undo SET undo = '' WHERE seq = ?1"
            } else {
                "DELETE FROM mutation_undo WHERE seq = ?1"
            };
            tx.prepare_cached(forget)?.execute([seq])?;
        }
        let left: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM mutation_undo WHERE undo <> '')",
            [],
            |row| row.get(0),
        )?;
        if !left && drop {
            tx.execute("DELETE FROM mutation_undo", [])?;
        }
        tx.commit()?;
        Ok(!left)
    }

    /// The writes the server answered that a rewind kept unapplied, in the order they were first
    /// applied, as JSON text of `[{id, name, args, at}]` that `applyCalls` takes.
    #[wasm_bindgen(js_name = answeredCalls)]
    pub fn answered_calls(&self) -> Result<String, JsError> {
        let calls: Vec<Call> = self
            .conn
            .prepare_cached(
                "SELECT call, name, args, at FROM mutation_undo WHERE undo = '' ORDER BY seq",
            )?
            .query_map([], |row| {
                let args: String = row.get(2)?;
                Ok(Call {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    args: serde_json::from_str(&args).unwrap_or_default(),
                    at: row.get::<_, i64>(3)? as f64,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(serde_json::to_string(&calls)?)
    }

    /// How many writes `applyCalls` applied that are not rewound.
    pub fn applied(&self) -> Result<u32, JsError> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM mutation_undo WHERE undo <> ''",
            [],
            |row| row.get(0),
        )?)
    }
}
