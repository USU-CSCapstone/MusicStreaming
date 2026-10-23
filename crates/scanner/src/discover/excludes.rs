//! A library's excludes: the globs, relative to a root, whose matches the walk leaves out
//! (`requirements/libraries.md` §1).

use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::scope::Scope;

/// Compile exclude globs. Patterns are relative to the root
/// (`requirements/libraries.md` §1).
pub fn compile_excludes(patterns: &[String]) -> Result<GlobSet, globset::Error> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        b.add(Glob::new(p)?);
    }
    b.build()
}

/// Whether `path`, which lies under `root`, matches `excludes` itself. Whether it lies in an
/// excluded folder is [`excluded_scope`]'s question.
pub fn excluded(root: &Path, path: &Path, excludes: &GlobSet) -> bool {
    !excludes.is_empty() && excludes.is_match(path.strip_prefix(root).unwrap_or(path))
}

/// Whether `scope` lies in an excluded folder: its own folder, or one between it and its root,
/// matches `excludes`. Nothing in such a scope is indexed. The root itself is never matched, as
/// the walk never matches it.
pub fn excluded_scope(scope: &Scope, excludes: &GlobSet) -> bool {
    let Ok(relative) = scope.path.strip_prefix(&scope.root) else {
        return false;
    };
    relative.ancestors().any(|dir| !dir.as_os_str().is_empty() && excludes.is_match(dir))
}
