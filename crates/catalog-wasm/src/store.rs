//! The device's files on OPFS, one database per library (`design/general.md` §7.1): a gzipped
//! snapshot inflated in with its hash taken as it lands, the names of what is stored, and removal. The
//! worker's sahpool handle is installed on first use and kept for the worker's life, since an
//! import borrows it.

use std::cell::Cell;
use std::io::{self, BufWriter, Write};

use flate2::write::GzDecoder;
use sha2::{Digest, Sha256};
use sqlite_wasm_rs::WasmOsCallback;
use sqlite_wasm_rs::vfs::VfsFilesManager;
use sqlite_wasm_rs::vfs::transfer::{DbImport, DbTransfer};
use sqlite_wasm_vfs::sahpool::{
    OpfsSAHError, OpfsSAHImportTarget, OpfsSAHPoolCfg, OpfsSAHPoolUtil, install,
};
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

/// Bytes of the inflated database written to OPFS at a time.
const WRITE_CHUNK: usize = 1 << 20;

/// What a damaged snapshot's errors say, which a caller tells from the device's own failures.
const DAMAGED: &str = "snapshot damaged";

/// A gzipped database file being inflated onto OPFS, which `finish` publishes and dropping aborts.
#[wasm_bindgen]
pub struct Import {
    inflate: GzDecoder<BufWriter<Target>>,
    /// The hash of the gzipped bytes written.
    hash: Sha256,
}

/// Where the inflated bytes go: the pool's import, which takes exactly the size it was begun with.
struct Target {
    import: DbImport<OpfsSAHImportTarget<'static>>,
    /// The bytes the import still takes.
    left: u64,
    /// The pool's own failure, kept so it is told apart from a damaged snapshot.
    fault: Option<OpfsSAHError>,
}

impl Write for Target {
    /// Hands `bytes` to the import. More bytes than it was begun with is a damaged snapshot.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let len = bytes.len() as u64;
        if len > self.left {
            return Err(io::Error::other("more bytes than the gzip trailer says"));
        }
        if let Err(error) = self.import.write(bytes) {
            self.fault = Some(error);
            return Err(io::Error::other("the import failed"));
        }
        self.left -= len;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[wasm_bindgen]
impl Import {
    /// Starts importing a gzipped database file as `name`. `size` is the inflated size, which the
    /// gzip trailer's last four bytes give (little-endian), so a database must be under 4 GiB: a
    /// larger one's size wraps and its import fails as damaged. One that is no database's size
    /// throws as a damaged snapshot. Nothing is visible under `name` until `finish`.
    pub async fn begin(name: String, size: u32) -> Result<Import, JsError> {
        if size < 512 || !size.is_multiple_of(512) {
            return Err(JsError::new(&format!("{DAMAGED}: {size} bytes is no database's size")));
        }
        let size = u64::from(size);
        let import = pool().await?.begin_import(&name, size)?;
        let target = Target { import, left: size, fault: None };
        let inflate = GzDecoder::new(BufWriter::with_capacity(WRITE_CHUNK, target));
        Ok(Import { inflate, hash: Sha256::new() })
    }

    /// Takes the gzip's next bytes. Bytes that do not inflate throw as a damaged snapshot.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), JsError> {
        self.hash.update(bytes);
        let written = self.inflate.write_all(bytes);
        written.map_err(|error| failure(self.inflate.get_mut().get_mut(), error))
    }

    /// Publishes the file once the whole gzip has been written. With `sha256` (lowercase hex),
    /// the file is published only if the gzip written hashes to it; otherwise, or if the gzip is
    /// cut short or its checksum fails, it throws and the import is discarded.
    pub fn finish(self, sha256: Option<String>) -> Result<(), JsError> {
        if let Some(expected) = sha256 {
            let actual: String = self.hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
            if actual != expected {
                return Err(JsError::new("snapshot hash mismatch"));
            }
        }
        let mut inflate = self.inflate;
        let finished = inflate.try_finish().and_then(|()| inflate.get_mut().flush());
        if let Err(error) = finished {
            return Err(failure(inflate.get_mut().get_mut(), error));
        }
        // Flushed, so nothing is left in the buffer; gzip's check of its length found it whole.
        let (target, _) = inflate.finish()?.into_parts();
        Ok(target.import.finish()?)
    }
}

/// The error to throw for `error` from the inflate: the pool's own failure if it had one, since
/// that is the device's, or else a damaged snapshot.
fn failure(target: &mut Target, error: io::Error) -> JsError {
    match target.fault.take() {
        Some(fault) => fault.into(),
        None => JsError::new(&format!("{DAMAGED}: {error}")),
    }
}
