//! The libraries the scanner indexes (`requirements/libraries.md` §1).

use std::path::PathBuf;

/// A library's id, as the store assigns it.
pub type LibraryId = i64;

/// What the scanner needs to know about a library (`requirements/libraries.md` §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryConfig {
    pub id: LibraryId,
    pub roots: Vec<PathBuf>,
    /// Glob patterns matched against paths relative to their root. A matching
    /// directory is not descended, and a scope at or inside it indexes nothing; a
    /// matching file is not indexed.
    pub excludes: Vec<String>,
}
