//! What the roots' watches do with an event (`design/scanning.md` §9): watch a folder it shows.

use std::path::Path;

use notify::EventKind;
use notify::event::{CreateKind, ModifyKind, RenameMode};

use super::{Roots, report};
use crate::worker::Stop;

impl Roots {
    /// Watches what an event of `kind` at `path` may show that is not yet watched: a folder
    /// made or moved in, with the folders below it. Stops walking once `stop` is asked.
    ///
    /// On macOS, an event does not say which side of a rename its path is, so any rename may
    /// have moved a folder in; a path moved away is gone, and watching it does nothing.
    pub(in crate::triggers) fn watch_shown(&mut self, kind: &EventKind, path: &Path, stop: &Stop) {
        if matches!(
            kind,
            EventKind::Create(CreateKind::Folder)
                | EventKind::Modify(ModifyKind::Name(RenameMode::To | RenameMode::Any))
        ) {
            self.watch_new(path, stop);
        }
    }

    /// Watches `folder`, which has just appeared in a root, with each folder below it.
    pub(super) fn watch_new(&mut self, folder: &Path, stop: &Stop) {
        if let Some(index) = self.holding(folder) {
            self.watch_in(index, folder, stop);
        }
    }

    /// Walks every root again, watching each folder not yet watched, as one made while the OS
    /// dropped events. Stops walking once `stop` is asked.
    pub(in crate::triggers) fn watch_all(&mut self, stop: &Stop) {
        for index in 0..self.roots.len() {
            let path = self.roots[index].path.clone();
            self.watch_in(index, &path, stop);
        }
    }

    /// Which root holds `path`.
    fn holding(&self, path: &Path) -> Option<usize> {
        self.roots.iter().position(|root| path.starts_with(&root.path))
    }

    /// Watches `folder` in the root at `index` ([`RootWatch::watch_folder`]). A folder that
    /// cannot be watched leaves the root not wholly watched, which is logged once.
    fn watch_in(&mut self, index: usize, folder: &Path, stop: &Stop) {
        let root = &mut self.roots[index];
        let Err(error) = root.watch.watch_folder(&root.path, folder, stop) else {
            return;
        };
        if root.watch.coverage.is_ok() {
            root.watch.coverage = Err(error);
            report(&root.path, &root.watch);
        }
    }
}
