//! Installing a plugin from a file or a URL, and uninstalling it.

use std::io::Write;

use jewelcase_plugins::Manifest;
use rusqlite::{Connection, params};
use tempfile::NamedTempFile;

use super::{PluginError, Plugins, grants, unexpected};
use crate::db::{Json, now_ms};

impl Plugins {
    /// Installs the plugin at `url`.
    pub async fn install_url(&self, url: String) -> Result<String, PluginError> {
        let bytes = jewelcase_plugins::download(&url).await?;
        self.install(bytes, Some(url)).await
    }

    /// Installs the plugin in `bytes`, disabled in every library and with the grants it held
    /// before for requests it makes again unchanged (`requirements/plugins.md` §4.2, §5), kept
    /// compiled for its runs, and returns its ID. A file that could never run here, or a plugin
    /// installed already, is refused.
    pub async fn install(
        &self,
        bytes: Vec<u8>,
        source_url: Option<String>,
    ) -> Result<String, PluginError> {
        if bytes.len() > jewelcase_plugins::MAX_SIZE {
            return Err(PluginError::Invalid("The file is over 50 MB.".into()));
        }
        let manifest = jewelcase_plugins::manifest::read(&bytes)?;
        let host = self.host()?;
        let (checker, dir) = (host.clone(), self.dir.clone());
        // Compiling is CPU-bound: 10 ms for a Rust plugin, about a second for JavaScript. A file
        // that checks out is then staged where it goes, also off the async threads.
        let (linked, staged) = tokio::task::spawn_blocking(move || -> Result<_, PluginError> {
            let linked = checker.check(&bytes).map_err(|e| {
                PluginError::Invalid(format!("This plugin cannot run here: {e:#}."))
            })?;
            std::fs::create_dir_all(&dir).map_err(internal)?;
            let mut staged = NamedTempFile::new_in(&dir).map_err(internal)?;
            staged.write_all(&bytes).map_err(internal)?;
            staged.as_file().sync_all().map_err(internal)?;
            Ok((linked, staged))
        })
        .await
        .map_err(unexpected)??;
        let (target, id) = (self.path(&manifest.id), manifest.id.clone());
        self.db
            .try_write(move |tx| {
                add(tx, &manifest, source_url.as_deref())?;
                // Put in place within the transaction, so a row never names a missing file. A
                // staged file not put in place is deleted as it is dropped.
                staged.persist(&target).map(drop).map_err(|error| internal(error.into()))
            })
            .await?;
        host.installed(&id, linked);
        Ok(id)
    }

    /// Uninstalls `id`, once each of its runs under way has ended: its row goes, the host forgets
    /// it, and its file is removed, all in one write. Files it wrote stay, and so do its grants
    /// and settings, for a reinstall (`requirements/plugins.md` §5). Its users' connections go
    /// with it, and so do the requests to forget their searches meant for it, so after a
    /// reinstall, which starts every position over, users connect again and nothing from before
    /// is sent twice.
    pub async fn uninstall(&self, id: String) -> Result<(), PluginError> {
        // Its turn in every library and at the server level, taken in order and held until it is
        // gone, so no run of it is under way meanwhile, and one waiting finds it gone.
        let levels = self.db.read(|conn| {
            let sql = "SELECT 0 UNION SELECT id FROM libraries ORDER BY 1";
            conn.prepare_cached(sql)?
                .query_map([], |row| row.get(0))?
                .collect::<Result<Vec<i64>, _>>()
        });
        let mut turns = Vec::new();
        for level in levels.await? {
            turns.push(self.turns.take(&id, level).await);
        }
        let path = self.path(&id);
        // A host not started yet has nothing linked to forget.
        let host = self.host.get().and_then(|host| host.as_ref().ok()).cloned();
        self.db
            .try_write(move |tx| {
                tx.execute(
                    "DELETE FROM search_events WHERE kind = 'stopped' AND plugin_id = ?1",
                    [&id],
                )?;
                if tx.execute("DELETE FROM plugins WHERE id = ?1", [&id])? == 0 {
                    return Err(PluginError::NotFound);
                }
                // Removed and forgotten within the transaction, as an install puts its file in
                // place, so an install of it again, which waits for this one, keeps its own.
                if let Some(host) = host {
                    host.uninstalled(&id);
                }
                match std::fs::remove_file(path) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        Err(internal(error))
                    }
                    _ => Ok(()),
                }
            })
            .await
    }
}

/// Records `manifest`'s plugin as installed from `source_url`, keeping only the grants it held
/// before whose requests it makes again unchanged ([`grants::keep_requested`]). Refused as
/// [`PluginError::Exists`], changing nothing, if a plugin with its ID is installed: the row is
/// never replaced, so what goes with it, such as its users' connections, goes only on a real
/// uninstall.
pub(super) fn add(
    conn: &Connection,
    manifest: &Manifest,
    source_url: Option<&str>,
) -> Result<(), PluginError> {
    let inserted = conn.execute(
        "INSERT INTO plugins (id, manifest, source_url, installed_at) \
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT (id) DO NOTHING",
        params![manifest.id, Json(manifest), source_url, now_ms()],
    )?;
    if inserted == 0 {
        return Err(PluginError::Exists(manifest.name.clone()));
    }
    Ok(grants::keep_requested(conn, manifest)?)
}

/// [`PluginError::Internal`] for a plugin's file that could not be stored or removed, once
/// `error` is logged.
fn internal(error: std::io::Error) -> PluginError {
    tracing::error!(%error, "cannot store a plugin");
    PluginError::Internal("The plugin could not be stored.")
}
