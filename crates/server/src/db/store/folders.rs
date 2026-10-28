//! What the index holds directly in one folder, which a scan reconciles the folder's listing
//! against (`design/database.md` §8).

use std::path::Path;

use jewelcase_scanner::{IndexedFile, IndexedFolder};
use rusqlite::{Result, Row, params};

use super::SqliteStore;
use super::roots::bounds_under;
use crate::db::DbError;

/// The rows of root `?1` from path `?2` on, and before `?3` when `bounded`, in path order, with
/// what the skip check needs.
fn select(bounded: bool) -> String {
    let below = if bounded { " AND path < ?3" } else { "" };
    format!(
        "SELECT path, id, file_size, file_mtime, missing_since, sidecar_stamp, tags_generation \
         FROM tracks \
         WHERE root_id = ?1 AND path >= ?2{below} ORDER BY path"
    )
}

/// What the index holds directly in the folder `dir` of `lib`; nothing when no root of `lib`
/// holds it. One pass over the `tracks_file` index in path order: each row directly in `dir` is
/// a file, and a row in a folder below names that folder, past whose rows the read then seeks.
/// It costs a seek per indexed folder in `dir` and a step per file, however much lies below, and
/// holds only what is directly in `dir`.
pub fn indexed_in(store: &SqliteStore, lib: i64, dir: &Path) -> Result<IndexedFolder, DbError> {
    let Some((root_id, rel)) = store.roots.locate(lib, dir) else {
        return Ok(IndexedFolder::default());
    };
    // The folder's paths start with `prefix`, up to `upper`; the root's are every path.
    let (prefix, upper) = match bounds_under(&rel) {
        Some([prefix, upper]) => (prefix, Some(upper)),
        None => (String::new(), None),
    };
    store.db.read_blocking(move |conn| {
        let mut stmt = conn.prepare_cached(&select(upper.is_some()))?;
        let mut folder = IndexedFolder::default();
        let mut from = prefix.clone();
        'seek: loop {
            let mut rows = match &upper {
                Some(upper) => stmt.query(params![root_id, from, upper])?,
                None => stmt.query(params![root_id, from])?,
            };
            while let Some(row) = rows.next()? {
                let path: String = row.get(0)?;
                let name = &path[prefix.len()..];
                if let Some((sub, _)) = name.split_once('/') {
                    // Past every path under `sub/`, which sort before `sub0`.
                    from = format!("{prefix}{sub}0");
                    folder.folders.push(sub.to_owned());
                    continue 'seek;
                }
                folder.files.insert(name.to_owned(), indexed_file(row)?);
            }
            return Ok(folder);
        }
    })
}

/// The [`IndexedFile`] in a row of [`select`].
fn indexed_file(row: &Row<'_>) -> Result<IndexedFile> {
    Ok(IndexedFile {
        track_id: row.get(1)?,
        size: row.get(2)?,
        mtime_ms: row.get(3)?,
        missing: row.get::<_, Option<i64>>(4)?.is_some(),
        sidecar_stamp: row.get(5)?,
        tags_generation: row.get(6)?,
    })
}

#[cfg(test)]
mod tests;
