//! The catalog's shared queries by name ([`jewelcase_catalog::query`]), answered from the device
//! database with the search index built from its documents.

use jewelcase_catalog::query::{self, number};
use jewelcase_catalog::search;
use jewelcase_core::Index;
use serde_json::Value;
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
        self.1 = Some(Index::build(search::documents(&self.0, library)?));
        Ok(())
    }

    /// Answers the operation `name` (an API operation's ID, such as `listTracks`) with `params`,
    /// a JSON object spelled as the API's query and path parameters, for `user` in `library`
    /// (decimal IDs). Returns the JSON the server sends. Throws on an unknown name, parameters
    /// the API would refuse, or a query that fails; `search` and search recipes throw until
    /// `buildIndex` has run.
    pub fn query(
        &self,
        user: &str,
        library: &str,
        name: &str,
        params: &str,
    ) -> Result<String, JsError> {
        let params: Value = serde_json::from_str(params).map_err(query::Error::from)?;
        let (user, library) = (number(user, "user")?, number(library, "library")?);
        Ok(query::answer(&self.0, user, library, name, params, self.1.as_ref())?.to_string())
    }
}
