//! The account's writes on the device database, through the code the server's handlers share
//! ([`jewelcase_catalog::mutate`]).

use jewelcase_catalog::mutate::{self, Undo};
use jewelcase_catalog::query::number;
use wasm_bindgen::prelude::*;

use super::Catalog;

#[wasm_bindgen]
impl Catalog {
    /// Applies the write `name` (`like`, `unlike`, `hide`, `unhide`, `createPlaylist`,
    /// `editPlaylist`, `appendToPlaylist` or `deletePlaylist`) with `args`, JSON text spelled as
    /// `jewelcase_catalog::mutate::apply` describes, for `user` in `library` (decimal IDs), at
    /// `now`, milliseconds since the epoch, which the caller keeps with the write so that
    /// applying it again makes the same rows. One transaction. Answers JSON text: `{"undo": …}`,
    /// which `revert` takes, or `{"refused": {"code", "message", "currentVersion"?}}` for a
    /// refusal (`not_found`, `version_conflict`, `id_taken`, `invalid`), with nothing changed. A
    /// database failure throws.
    pub fn mutate(
        &self,
        user: &str,
        library: &str,
        name: &str,
        args: &str,
        now: f64,
    ) -> Result<String, JsError> {
        self.totals.clear();
        let (user, library) = (number(user, "user")?, number(library, "library")?);
        let answer = match mutate::apply(&self.conn, user, library, name, args, now as i64) {
            Ok(undo) => serde_json::json!({ "undo": undo }),
            Err(error) => match error.refusal() {
                Some(refusal) => serde_json::json!({ "refused": refusal }),
                None => return Err(error.into()),
            },
        };
        Ok(answer.to_string())
    }

    /// Puts back the rows an `undo` from `mutate` kept, exactly as they were, in one transaction.
    pub fn revert(&self, undo: &str) -> Result<(), JsError> {
        self.totals.clear();
        let undo: Undo = serde_json::from_str(undo)?;
        Ok(mutate::revert(&self.conn, &undo)?)
    }
}
