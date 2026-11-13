//! The catalog's shared queries by name ([`jewelcase_catalog::query`]), answered from the device
//! database with the search index built from its documents.

use jewelcase_catalog::query::{self, number};
use jewelcase_catalog::search;
use jewelcase_core::{Finishing, IndexBuilder};
use wasm_bindgen::prelude::*;

use super::Catalog;

/// A search index under construction: the documents read into a builder, then the builder's sorts.
pub(crate) enum Building {
    Reading { library: i64, reading: search::Reading, builder: IndexBuilder },
    Finishing(Finishing),
}

#[wasm_bindgen]
impl Catalog {
    /// Builds the search index of `library` (the file's own) from the device's documents,
    /// replacing an earlier one, which is let go first, so that the two are never held at once.
    /// If the build fails there is none. The caller decides when: the index is not kept in step
    /// with `commit`. Holds the worker until it is done; see `indexBegin` for a build in parts.
    #[wasm_bindgen(js_name = buildIndex)]
    pub fn build_index(&mut self, library: &str) -> Result<(), JsError> {
        self.index_begin(library)?;
        while !self.index_step(u32::MAX)? {}
        Ok(())
    }

    /// Starts building the search index of `library` (the file's own) in parts, which
    /// `indexStep` does one after another: the earlier index, and any build under way, are let
    /// go. The rows must not change before the last part, or the index is of neither version: the
    /// caller begins again then.
    #[wasm_bindgen(js_name = indexBegin)]
    pub fn index_begin(&mut self, library: &str) -> Result<(), JsError> {
        let library = number(library, "library")?;
        self.index = None;
        self.building = Some(Building::Reading {
            library,
            reading: search::Reading::default(),
            builder: IndexBuilder::default(),
        });
        Ok(())
    }

    /// Does one part of the build `indexBegin` started, about `limit` documents' or sorted
    /// elements' worth of work, and says whether the index is now built. If it fails there is
    /// none, and no build under way.
    #[wasm_bindgen(js_name = indexStep)]
    pub fn index_step(&mut self, limit: u32) -> Result<bool, JsError> {
        let limit = limit as usize;
        let Some(building) = self.building.take() else {
            return Err(JsError::new("no index is being built"));
        };
        match building {
            Building::Reading { library, mut reading, mut builder } => {
                let all =
                    search::documents_from(&self.conn, library, &mut reading, limit, |document| {
                        builder.add(document)
                    })?;
                self.building = Some(if all {
                    Building::Finishing(builder.finishing())
                } else {
                    Building::Reading { library, reading, builder }
                });
                Ok(false)
            }
            Building::Finishing(mut finishing) => {
                if finishing.step(limit) {
                    self.index = Some(finishing.into_index());
                    return Ok(true);
                }
                self.building = Some(Building::Finishing(finishing));
                Ok(false)
            }
        }
    }

    /// Lets go of the search index, if there is one, and of a build under way, so another
    /// library's can take its memory.
    #[wasm_bindgen(js_name = dropIndex)]
    pub fn drop_index(&mut self) {
        self.index = None;
        self.building = None;
    }

    /// Answers the operation `name` (an API operation's ID, such as `listTracks`) with `params`,
    /// a JSON object spelled as the API's query and path parameters, for `user` in `library`
    /// (decimal IDs). Returns the JSON text the server sends, serialized once and passed to JS as
    /// it is. Throws on an unknown name, parameters the API would refuse, or a query that fails;
    /// `search` and search recipes throw until `buildIndex` has run. A list's counted total is
    /// remembered until the next `stage`, `commit` or `sql`.
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
