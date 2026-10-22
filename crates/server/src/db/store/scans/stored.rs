//! How a scan row keeps its scopes and cursor: each root by id, and each path relative to it.

use std::path::Path;

use jewelcase_scanner::{Depth, Scan, Scope};
use rusqlite::Error::{FromSqlConversionFailure, ToSqlConversionFailure};
use rusqlite::types::Type;
use rusqlite::{Result, Row};
use serde::{Deserialize, Serialize};

use crate::db::Json;
use crate::db::store::roots::{Roots, join, relative};

/// A scope as a scan row keeps it: its root by id, and its folder relative to that root.
#[derive(Serialize, Deserialize)]
pub struct StoredScope {
    root: i64,
    path: String,
    depth: Depth,
}

/// `scopes` as a scan row keeps them, in JSON. Fails when a scope's root is not one of `lib`'s
/// active roots, or its folder is not UTF-8 text under that root: better no record of a scan than
/// one that resumes as a scan of something else.
pub fn stored_scopes(roots: &Roots, lib: i64, scopes: &[Scope]) -> Result<Json<Vec<StoredScope>>> {
    let stored = scopes.iter().map(|scope| {
        let root =
            roots.root_by_path(lib, &scope.root).ok_or("a scope's root is not the library's")?;
        let path = scope.path.strip_prefix(&scope.root).ok().and_then(Path::to_str);
        let path = path.ok_or("a scope's folder is not text under its root")?;
        Ok(StoredScope { root, path: path.to_owned(), depth: scope.depth })
    });
    let stored = stored.collect::<Result<_, &str>>();
    stored.map(Json).map_err(|why| ToSqlConversionFailure(why.into()))
}

/// The scopes kept in column `index` of row `r`, with their roots among `lib`'s active `roots`.
/// A scope whose root is not one of them cannot be read.
pub fn scopes(r: &Row, index: usize, roots: &Roots, lib: i64) -> Result<Vec<Scope>> {
    let Json(stored): Json<Vec<StoredScope>> = r.get(index)?;
    stored
        .into_iter()
        .map(|scope| {
            let root = roots.root(lib, scope.root).ok_or_else(|| {
                let why = "a scope's root is no longer the library's";
                FromSqlConversionFailure(index, Type::Text, why.into())
            })?;
            Ok(Scope { path: join(&root, &scope.path), root, depth: scope.depth })
        })
        .collect()
}

/// The directory `scan`'s cursor resumes after, if any, relative to the root of the scope it is
/// in. A directory whose path is not UTF-8 is kept as its nearest ancestor that is, since a lossy
/// spelling could sort past folders not yet scanned; a resume then walks the folders between that
/// ancestor and the real cursor again.
pub fn cursor_dir(scan: &Scan) -> Option<String> {
    let cursor = scan.cursor.as_ref()?;
    let scope = scope_at(&scan.scopes, cursor.scope_index)?;
    let dir = cursor.after_directory.as_deref()?.ancestors().find_map(Path::to_str)?;
    Some(relative(&scope.root, Path::new(dir)))
}

/// The scope at `index` in `scopes`, if there is one.
pub fn scope_at(scopes: &[Scope], index: i64) -> Option<&Scope> {
    scopes.get(usize::try_from(index).ok()?)
}
