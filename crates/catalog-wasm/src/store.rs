//! The device's files on OPFS, one database per library (`design/general.md` §7.1): a snapshot
//! streamed in with its hash taken as it lands, the names of what is stored, and removal. The
//! worker's sahpool handle is installed on first use and kept for the worker's life, since an
//! import borrows it.

use std::cell::Cell;

use sha2::{Digest, Sha256};
use sqlite_wasm_rs::WasmOsCallback;
use sqlite_wasm_rs::vfs::VfsFilesManager;
use sqlite_wasm_rs::vfs::transfer::{DbImport, DbTransfer};
use sqlite_wasm_vfs::sahpool::{OpfsSAHImportTarget, OpfsSAHPoolCfg, OpfsSAHPoolUtil, install};
use wasm_bindgen::prelude::*;

thread_local! {
    static POOL: Cell<Option<&'static OpfsSAHPoolUtil>> = const { Cell::new(None) };
}

/// The worker's sahpool handle. Throws where OPFS is unavailable.
async fn pool() -> Result<&'static OpfsSAHPoolUtil, JsError> {
    if let Some(pool) = POOL.get() {
        return Ok(pool);
    }
    let util = install::<WasmOsCallback>(&OpfsSAHPoolCfg::default(), false).await?;
    let pool: &'static OpfsSAHPoolUtil = Box::leak(Box::new(util));
    POOL.set(Some(pool));
    Ok(pool)
}

/// The names of the database files stored, journals included. Throws where OPFS is unavailable.
#[wasm_bindgen(js_name = storeNames)]
pub async fn store_names() -> Result<Vec<String>, JsError> {
    Ok(pool().await?.names()?)
}

/// Deletes the file `name` if there is one, and says whether there was. No connection may have it
/// open.
#[wasm_bindgen(js_name = storeRemove)]
pub async fn store_remove(name: String) -> Result<bool, JsError> {
    Ok(pool().await?.remove(&name)?)
}

/// Makes room for at least `files` files at once, journals included, which the pool cannot grow
/// during an import. Call it between imports.
#[wasm_bindgen(js_name = storeReserve)]
pub async fn store_reserve(files: usize) -> Result<(), JsError> {
    Ok(pool().await?.ensure_capacity(files).await?)
}

/// Releases the pool's OPFS handles so another tab's worker can take them at once. The worker
/// must not use the pool afterwards. Close every `Catalog` first. Does nothing if the pool was
/// never installed.
#[wasm_bindgen(js_name = storePause)]
pub fn store_pause() -> Result<(), JsError> {
    if let Some(pool) = POOL.get() {
        pool.pause()?;
    }
    Ok(())
}

/// A database file being streamed onto OPFS, which `finish` publishes and dropping aborts.
#[wasm_bindgen]
pub struct Import(DbImport<OpfsSAHImportTarget<'static>>, Sha256);

#[wasm_bindgen]
impl Import {
    /// Starts importing a database file of `size` bytes (under 4 GiB) as `name`. Nothing is
    /// visible under `name` until `finish`.
    pub async fn begin(name: String, size: u32) -> Result<Import, JsError> {
        let target = pool().await?.begin_import(&name, u64::from(size))?;
        Ok(Import(target, Sha256::new()))
    }

    /// Appends the file's next bytes.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), JsError> {
        self.0.write(bytes)?;
        self.1.update(bytes);
        Ok(())
    }

    /// Publishes the file once every byte has been written. With `sha256` (lowercase hex), the
    /// file is published only if the bytes written hash to it; otherwise it throws, and the
    /// import is discarded.
    pub fn finish(self, sha256: Option<String>) -> Result<(), JsError> {
        if let Some(expected) = sha256 {
            let actual: String = self.1.finalize().iter().map(|b| format!("{b:02x}")).collect();
            if actual != expected {
                return Err(JsError::new("snapshot hash mismatch"));
            }
        }
        Ok(self.0.finish()?)
    }
}
