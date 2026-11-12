//! Recording, removing and clearing recent searches (`requirements/search.md` §6).

use jewelcase_core::words;
use rusqlite::params;

use super::{Effect, Error, Writer};
use crate::recent_searches::{KEPT, KINDS};
use crate::sync::Kind;

/// The longest query kept, in bytes.
pub const MAX_QUERY: usize = 500;

/// `query` as it is kept, trimmed, and its words as search compares them, or why it cannot be
/// kept: it is over [`MAX_QUERY`] bytes, or has nothing in it to search for.
pub fn check_query(query: &str) -> Result<(String, String), Error> {
    let query = query.trim();
    if query.len() > MAX_QUERY {
        return Err(Error::Invalid(format!("query must be at most {MAX_QUERY} bytes.")));
    }
    let folded = words(query).join(" ");
    if folded.is_empty() {
        return Err(Error::Invalid("query has nothing in it to search for.".to_owned()));
    }
    Ok((query.to_owned(), folded))
}

impl Writer<'_> {
    /// Records `query` as the user's newest search in the library under `id`, with the result
    /// they acted on, `selected` as its kind and ID, and replaces an earlier one of the same words;
    /// only the [`KEPT`] newest stay. A query [`check_query`] refuses, an `id` that is not
    /// positive, and a `selected` that is not a track, album or artist in the library are
    /// [`Error::Invalid`]; an `id` any user's search has is [`Error::IdTaken`]. Each search
    /// replaced is reported before the new one, and each pushed out after it.
    pub fn record_search(
        &mut self,
        id: i64,
        query: &str,
        selected: Option<(&str, i64)>,
    ) -> Result<(), Error> {
        let (query, folded) = check_query(query)?;
        if id <= 0 {
            return Err(Error::Invalid("id must be a positive 63-bit integer.".to_owned()));
        }
        let selected = match selected {
            None => None,
            Some((kind, selected)) => {
                let Some((kind, table)) = KINDS.iter().find(|(k, _)| *k == kind) else {
                    return Err(Error::Invalid(
                        "selected must be a track, album, or artist.".to_owned(),
                    ));
                };
                let sql = format!("SELECT 1 FROM {table} WHERE id = ?1 AND library_id = ?2");
                if !self.conn.prepare_cached(&sql)?.exists([selected, self.library])? {
                    return Err(Error::Invalid("selected is not in this library.".to_owned()));
                }
                Some((*kind, selected))
            }
        };
        if self.conn.prepare_cached("SELECT 1 FROM recent_searches WHERE id = ?1")?.exists([id])? {
            return Err(Error::IdTaken);
        }
        let same: Vec<i64> = self
            .conn
            .prepare_cached(
                "SELECT id FROM recent_searches WHERE user_id = ?1 AND library_id = ?2 \
                 AND folded = ?3",
            )?
            .query_map(params![self.user, self.library, folded], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for old in same {
            self.drop_search(old, false)?;
        }
        self.touch(Kind::RecentSearch, id, 0)?;
        let (kind, selected) = selected.unzip();
        self.conn
            .prepare_cached(
                "INSERT INTO recent_searches (id, user_id, library_id, query, folded, \
                 selected_type, selected_id, searched_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?
            .execute(params![
                id,
                self.user,
                self.library,
                query,
                folded,
                kind,
                selected,
                self.now
            ])?;
        self.report(Effect::Search { id, kept: true, removed: false })?;
        let older: Vec<i64> = self
            .conn
            .prepare_cached(
                "SELECT id FROM recent_searches WHERE user_id = ?1 AND library_id = ?2 \
                 ORDER BY searched_at DESC, id DESC LIMIT -1 OFFSET ?3",
            )?
            .query_map(params![self.user, self.library, KEPT], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for old in older {
            self.drop_search(old, false)?;
        }
        Ok(())
    }

    /// Removes the user's recent search `id` in the library, and says whether there was one.
    pub fn delete_search(&mut self, id: i64) -> Result<bool, Error> {
        let theirs = self
            .conn
            .prepare_cached(
                "SELECT 1 FROM recent_searches WHERE id = ?1 AND user_id = ?2 AND library_id = ?3",
            )?
            .exists(params![id, self.user, self.library])?;
        if theirs {
            self.drop_search(id, true)?;
        }
        Ok(theirs)
    }

    /// Removes all the user's recent searches in the library.
    pub fn clear_searches(&mut self) -> Result<(), Error> {
        let all: Vec<i64> = self
            .conn
            .prepare_cached(
                "SELECT id FROM recent_searches WHERE user_id = ?1 AND library_id = ?2 ORDER BY id",
            )?
            .query_map(params![self.user, self.library], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in all {
            self.drop_search(id, true)?;
        }
        Ok(())
    }

    /// Deletes the recent search `id`, which is the user's in the library, and reports it as
    /// `removed` by the user, or replaced or pushed out.
    fn drop_search(&mut self, id: i64, removed: bool) -> Result<(), Error> {
        self.touch(Kind::RecentSearch, id, 0)?;
        self.conn.prepare_cached("DELETE FROM recent_searches WHERE id = ?1")?.execute([id])?;
        self.report(Effect::Search { id, kept: false, removed })?;
        Ok(())
    }
}
