//! The catalog's shared queries by name ([`jewelcase_catalog::query`]), answered from the device
//! database with the search index built from its documents.

use jewelcase_catalog::query::{self, number};
use jewelcase_catalog::search;
use jewelcase_core::Index;
use wasm_bindgen::prelude::*;

use super::Catalog;

#[wasm_bindgen]
impl Catalog {
    /// Builds the search index of `library` (the file's own) from the device's documents,
    /// replacing an earlier one. The caller decides when: the index is not kept in step with
    /// `commit`.
    #[wasm_bindgen(js_name = buildIndex)]
    pub fn build_index(&mut self, library: &str) -> Result<(), JsError> {
        let library = number(library, "library")?;
        self.index = Some(Index::build(search::documents(&self.conn, library)?));
        Ok(())
    }

    /// Answers the operation `name` (an API operation's ID, such as `listTracks`) with `params`,
    /// a JSON object spelled as the API's query and path parameters, for `user` in `library`
    /// (decimal IDs). Returns the JSON text the server sends, serialized once and passed to JS as
    /// it is. Throws on an unknown name, parameters the API would refuse, or a query that fails;
    /// `search` and search recipes throw until `buildIndex` has run. A list's counted total is
    /// remembered until the next `stage` or `commit`.
    pub fn query(
        &self,
        user: &str,
        library: &str,
        name: &str,
        params: &str,
    ) -> Result<String, JsError> {
        let (user, library) = (number(user, "user")?, number(library, "library")?);
        Ok(query::answer(
            &self.conn,
            user,
            library,
            name,
            params,
            self.index.as_ref(),
            Some(&self.totals),
        )?)
    }
}
