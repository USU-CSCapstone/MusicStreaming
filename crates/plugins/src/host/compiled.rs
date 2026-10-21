//! Each installed plugin compiled once (`design/plugins.md` §2): kept in memory by ID until it
//! is uninstalled or installed again, since compiling is what costs and hooks run a plugin
//! often. The engine's disk cache, when it has one, keeps the compiled code across restarts.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio::sync::OnceCell;
use wasmtime::component::Component;

use super::Host;

/// Each plugin's component by ID. A cell left empty is filled by the first run to ask, while
/// runs that ask meanwhile wait for that one compile.
pub(super) type Cache = Mutex<HashMap<String, Arc<OnceCell<Component>>>>;

impl Host {
    /// Keeps `component`, from [`Host::check`], as plugin `id`'s, in place of any compiled
    /// before. Called once `id` is installed, so a reinstall never runs the old code.
    pub fn installed(&self, id: &str, component: Component) {
        let cell = Arc::new(OnceCell::from(component));
        self.compiled.lock().expect("cache lock").insert(id.to_owned(), cell);
    }

    /// Forgets plugin `id`'s compiled component. Called once it is uninstalled.
    pub fn uninstalled(&self, id: &str) {
        self.compiled.lock().expect("cache lock").remove(id);
    }

    /// Plugin `id`, installed at `file`, compiled: kept from its install or an earlier run, or
    /// else compiled now, once however many runs ask at the same time. A compile that fails is
    /// not kept, so the next run tries again.
    pub(super) async fn compiled(&self, id: &str, file: &Path) -> Result<Component, String> {
        let cell =
            self.compiled.lock().expect("cache lock").entry(id.to_owned()).or_default().clone();
        let compile = || async {
            // Compiling is CPU-bound, up to a second for a JavaScript plugin that the disk
            // cache does not have.
            let (engine, file) = (self.engine.clone(), file.to_path_buf());
            tokio::task::spawn_blocking(move || Component::from_file(&engine, file))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.root_cause().to_string())
        };
        let component = cell.get_or_try_init(compile).await;
        component.cloned().map_err(|e| format!("the plugin could not be loaded: {e}"))
    }
}
