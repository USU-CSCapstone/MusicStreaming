//! Each installed plugin compiled and linked once (`design/plugins.md` §2): kept in memory by ID,
//! with its HTTP client, until it is uninstalled or installed again, since compiling is what
//! costs and hooks run a plugin often. The engine's disk cache, when it has one, keeps the
//! compiled code across restarts.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio::sync::OnceCell;
use wasmtime::component::Component;

use super::http::KeptClient;
use super::{Host, Library, PluginPre, Run};
use crate::lock::Recover;

/// Each plugin's entry, by ID.
pub(super) type Cache<L> = Mutex<HashMap<String, Arc<Entry<L>>>>;

/// What a plugin's runs share until it is uninstalled or installed again, which replaces the
/// entry whole: a run under way keeps the one it started with, so whatever it adds is forgotten
/// with it.
pub(super) struct Entry<L: Library> {
    /// Its component, linked for a run over an `L`. Left empty, it is filled by the first run to
    /// ask, while runs that ask meanwhile wait for that one compile.
    linked: OnceCell<PluginPre<Run<L>>>,
    /// Its HTTP client, kept beside its code so that its runs, in any library, reuse its
    /// connections, and an uninstall or reinstall forgets it with the code.
    client: Arc<KeptClient>,
}

impl<L: Library> Host<L> {
    /// Keeps `linked`, from [`Host::check`], as plugin `id`'s, in place of any linked before
    /// and of its HTTP client. Called once `id` is installed, so a reinstall never runs the old
    /// code.
    pub fn installed(&self, id: &str, linked: PluginPre<Run<L>>) {
        let entry = Entry { linked: OnceCell::from(linked), client: Arc::default() };
        self.compiled.lock().recover().insert(id.to_owned(), Arc::new(entry));
    }

    /// Forgets plugin `id`'s linked component and its HTTP client. Called once it is
    /// uninstalled.
    pub fn uninstalled(&self, id: &str) {
        self.compiled.lock().recover().remove(id);
    }

    /// Plugin `id`, installed at `file`, compiled and linked, and where its HTTP client is kept.
    /// The component is kept from its install or an earlier run, or else made now, once however
    /// many runs ask at the same time. One that fails is not kept, so the next run tries again.
    /// The file is read here and compiled from its bytes, so no error names where it is: a
    /// failure to load reaches admins as the run's summary, which carries no storage path.
    pub(super) async fn compiled(
        &self,
        id: &str,
        file: &Path,
    ) -> wasmtime::Result<(PluginPre<Run<L>>, Arc<KeptClient>)> {
        let empty = || Arc::new(Entry { linked: OnceCell::new(), client: Arc::default() });
        let entry =
            self.compiled.lock().recover().entry(id.to_owned()).or_insert_with(empty).clone();
        let link = || async {
            // Compiling is CPU-bound, up to a second for a JavaScript plugin that the disk
            // cache does not have.
            let (engine, file) = (self.engine.clone(), file.to_path_buf());
            let compiled =
                tokio::task::spawn_blocking(move || Component::new(&engine, std::fs::read(file)?));
            self.pre(&compiled.await??)
        };
        let linked = entry.linked.get_or_try_init(link).await?.clone();
        Ok((linked, entry.client.clone()))
    }
}
