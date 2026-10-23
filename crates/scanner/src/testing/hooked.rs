//! A store wrapper with hooks, for scenarios where the store is slow or fails, the world changes
//! partway through a scan, or scans are held up while the queue is worked.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use crate::library::LibraryId;
use crate::scan::{Scan, ScanState, Trigger};
use crate::scope::Scope;
use crate::store::{Batch, IndexedFolder, Store, StoreError};

/// A hook run before a call.
type Hook<'a> = Box<dyn Fn() -> Result<(), StoreError> + Send + Sync + 'a>;

/// A [`Hook`] given what the call is about, such as the batch it applies.
type HookOn<'a, A> = Box<dyn Fn(&A) -> Result<(), StoreError> + Send + Sync + 'a>;

/// Passes every call to the store it wraps, first running the hook set for it, so a test can
/// change the world, hold a call up, or fail it. Every hook passes until a builder method such as
/// [`Hooked::on_apply`] sets it, and one that returns an error fails the call before it reaches
/// the inner store. Hooks may borrow from the test for `'a`. Apart from the hooks,
/// [`Hooked::hold`] holds scans up until [`Hooked::release`].
pub struct Hooked<'a> {
    inner: Arc<dyn Store>,
    on_apply: HookOn<'a, Batch>,
    on_folder: HookOn<'a, Path>,
    on_under: Hook<'a>,
    on_scan_write: Hook<'a>,
    held: Mutex<bool>,
    released: Condvar,
}

impl<'a> Hooked<'a> {
    /// Wraps `inner`, with every hook passing and no scan held up.
    pub fn new(inner: Arc<dyn Store>) -> Hooked<'a> {
        Hooked {
            inner,
            on_apply: Box::new(|_| Ok(())),
            on_folder: Box::new(|_| Ok(())),
            on_under: Box::new(|| Ok(())),
            on_scan_write: Box::new(|| Ok(())),
            held: Mutex::new(false),
            released: Condvar::new(),
        }
    }

    /// Runs `hook` with each batch before it is applied.
    pub fn on_apply(
        self,
        hook: impl Fn(&Batch) -> Result<(), StoreError> + Send + Sync + 'a,
    ) -> Hooked<'a> {
        Hooked { on_apply: Box::new(hook), ..self }
    }

    /// Runs `hook` with each folder before what the index holds in it is read
    /// ([`Store::indexed_in`]), which a scan does as it starts on the folder, and for each folder
    /// under one gone from its parent's listing, as it marks what that folder held missing.
    pub fn on_folder(
        self,
        hook: impl Fn(&Path) -> Result<(), StoreError> + Send + Sync + 'a,
    ) -> Hooked<'a> {
        Hooked { on_folder: Box::new(hook), ..self }
    }

    /// Runs `hook` before each [`Store::problems_under`] and [`Store::indexes_under`].
    pub fn on_under(
        self,
        hook: impl Fn() -> Result<(), StoreError> + Send + Sync + 'a,
    ) -> Hooked<'a> {
        Hooked { on_under: Box::new(hook), ..self }
    }

    /// Runs `hook` before each scan row is created or saved on its own ([`Store::update_scan`]).
    /// A row saved with a batch is part of the batch, which [`Hooked::on_apply`]'s hook sees.
    pub fn on_scan_write(
        self,
        hook: impl Fn() -> Result<(), StoreError> + Send + Sync + 'a,
    ) -> Hooked<'a> {
        Hooked { on_scan_write: Box::new(hook), ..self }
    }

    /// Holds up each scan as it next saves its row as running on its own ([`Store::update_scan`]),
    /// which it does as it starts: the row is written, then the save waits until
    /// [`Hooked::release`], so a scan that starts while held waits before it reads anything.
    pub fn hold(&self) {
        *self.held.lock().unwrap() = true;
    }

    /// Lets every held scan go on.
    pub fn release(&self) {
        *self.held.lock().unwrap() = false;
        self.released.notify_all();
    }
}

impl Store for Hooked<'_> {
    /// The inner store's new scan, once the scan-write hook passes.
    fn create_scan(
        &self,
        library: LibraryId,
        trigger: Trigger,
        scopes: &[Scope],
    ) -> Result<Scan, StoreError> {
        (self.on_scan_write)()?;
        self.inner.create_scan(library, trigger, scopes)
    }
    /// Saves `scan` in the inner store once the scan-write hook passes, then, for a running
    /// scan while [`Hooked::hold`] is in force, waits for [`Hooked::release`].
    fn update_scan(&self, scan: &Scan) -> Result<(), StoreError> {
        (self.on_scan_write)()?;
        self.inner.update_scan(scan)?;
        if scan.state == ScanState::Running {
            let held = self.held.lock().unwrap();
            drop(self.released.wait_while(held, |held| *held).unwrap());
        }
        Ok(())
    }
    /// The inner store's, with no hook.
    fn interrupted_scans(&self, library: LibraryId) -> Result<Vec<Scan>, StoreError> {
        self.inner.interrupted_scans(library)
    }
    /// The inner store's, once the folder hook passes `dir`.
    fn indexed_in(&self, library: LibraryId, dir: &Path) -> Result<IndexedFolder, StoreError> {
        (self.on_folder)(dir)?;
        self.inner.indexed_in(library, dir)
    }
    /// Applies `batch` to the inner store once the apply hook passes it.
    fn apply(&self, library: LibraryId, batch: Batch) -> Result<i64, StoreError> {
        (self.on_apply)(&batch)?;
        self.inner.apply(library, batch)
    }
    /// The inner store's, once the under hook passes.
    fn problems_under(
        &self,
        library: LibraryId,
        scope: &Scope,
    ) -> Result<Vec<PathBuf>, StoreError> {
        (self.on_under)()?;
        self.inner.problems_under(library, scope)
    }
    /// The inner store's, once the under hook passes.
    fn indexes_under(&self, library: LibraryId, folder: &Path) -> Result<bool, StoreError> {
        (self.on_under)()?;
        self.inner.indexes_under(library, folder)
    }
}
