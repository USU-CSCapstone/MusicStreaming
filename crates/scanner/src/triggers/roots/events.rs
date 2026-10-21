//! What the roots' watches do with an event (`design/scanning.md` §9): watch a folder it shows,
//! or one it may have made readable.

use std::path::Path;

use notify::EventKind;
use notify::event::{CreateKind, ModifyKind, RenameMode};

use super::{Events, RootWatch, Roots, report};

impl<W: Fn(&Path, &Events) -> RootWatch> Roots<W> {
    /// Watches what an event of `kind` at `path` may show that is not yet watched: a folder
    /// made or moved in, with the folders below it, or, when permissions changed, a folder at or
    /// below `path` that could not be read before.
    pub(in crate::triggers) fn watch_shown(&mut self, kind: &EventKind, path: &Path) {
        match kind {
            EventKind::Create(CreateKind::Folder)
            | EventKind::Modify(ModifyKind::Name(RenameMode::To)) => self.watch_new(path),
            EventKind::Modify(ModifyKind::Metadata(_)) => self.watch_readable(path),
            _ => {}
        }
    }

    /// Watches `folder`, which has just appeared in a root, with each folder below it.
    pub(super) fn watch_new(&mut self, folder: &Path) {
        if let Some(index) = self.holding(folder) {
            self.watch_in(index, folder);
        }
    }

    /// Watches each folder at or below `folder` that could not be read when last tried
    /// ([`RootWatch::unreadable_under`]), as a change to `folder`'s permissions may have made
    /// it readable. Nothing else is walked again.
    fn watch_readable(&mut self, folder: &Path) {
        let Some(index) = self.holding(folder) else {
            return;
        };
        for unread in self.roots[index].watch.unreadable_under(folder) {
            self.watch_in(index, &unread);
        }
    }

    /// Walks every root again, watching each folder not yet watched, as one made while the OS
    /// dropped events.
    pub(in crate::triggers) fn watch_all(&mut self) {
        for index in 0..self.roots.len() {
            let path = self.roots[index].path.clone();
            self.watch_in(index, &path);
        }
    }

    /// Which root holds `path`.
    fn holding(&self, path: &Path) -> Option<usize> {
        self.roots.iter().position(|root| path.starts_with(&root.path))
    }

    /// Watches `folder` in the root at `index` ([`RootWatch::watch_folder`]). A folder that
    /// cannot be watched leaves the root not wholly watched: watched afresh at a later check,
    /// or, when the OS limit on watches stopped it, left to the fallback schedule, as a fresh
    /// watch would meet the limit too.
    fn watch_in(&mut self, index: usize, folder: &Path) {
        let root = &mut self.roots[index];
        let Err(error) = root.watch.watch_folder(&root.path, folder) else {
            return;
        };
        if root.watch.coverage.is_ok() {
            root.watch.coverage = Err(error);
            report(&root.path, &root.watch);
            self.fall_back();
        }
    }
}
