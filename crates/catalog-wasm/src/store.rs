//! The device's files on OPFS, one database per library (`design/general.md` §7.1): a gzipped
//! snapshot inflated in with its hash taken as it lands, the names of what is stored, and removal.
//! The worker's sahpool handle is installed on first use and kept for the worker's life, since an
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
    /// The worker's sahpool handle, set once the pool is installed.
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

/// How much of a database file holds the facts its size is read from: the 100-byte header.
const HEADER: usize = 100;

/// Where the inflated bytes go: the pool's import, begun once the header says how big it is.
struct Target {
    pool: &'static OpfsSAHPoolUtil,
    name: String,
    /// The bytes written before the import began, up to the write that completes the header.
    head: Vec<u8>,
    import: Option<DbImport<OpfsSAHImportTarget<'static>>>,
    /// The pool's own failure, kept so it is told apart from a damaged snapshot.
    fault: Option<OpfsSAHError>,
}

/// Hands `bytes` to `import`, which takes exactly the size the header gave. The import's own
/// refusals, such as more bytes than that or a header that is not a database's, are a damaged
/// snapshot; the pool's are not, and are kept in `fault`.
fn put(
    import: &mut DbImport<OpfsSAHImportTarget<'static>>,
    fault: &mut Option<OpfsSAHError>,
    bytes: &[u8],
) -> io::Result<()> {
    import.write(bytes).map_err(|error| match error {
        OpfsSAHError::Transfer(_) => io::Error::other(error.to_string()),
        error => {
            *fault = Some(error);
            io::Error::other("the import failed")
        }
    })
}

impl Write for Target {
    /// Keeps the bytes until the header is whole, then begins the import it sizes and writes them
    /// and the rest to it.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(import) = &mut self.import {
            put(import, &mut self.fault, bytes)?;
            return Ok(bytes.len());
        }
        self.head.extend_from_slice(bytes);
        if self.head.len() >= HEADER {
            let head = std::mem::take(&mut self.head);
            let import = match self.pool.begin_import(&self.name, page_count_size(&head)?) {
                Ok(import) => import,
                Err(error @ OpfsSAHError::Transfer(_)) => {
                    return Err(io::Error::other(error.to_string()));
                }
                Err(error) => {
                    self.fault = Some(error);
                    return Err(io::Error::other("the import failed"));
                }
            };
            put(self.import.insert(import), &mut self.fault, &head)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The size in bytes of the database whose first 100 bytes are `header`: its page size (offset 16,
/// where 1 means 65,536) times the page count in its header (offset 28). That count counts only
/// if the header's "version-valid-for" number (offset 92) is the file change counter (offset 24),
/// which says the last writer kept it up; otherwise it is an error. Whether the result is a
/// database's size, and the header a database's, is for the import to refuse.
fn page_count_size(header: &[u8]) -> io::Result<u64> {
    let number = |at: usize| {
        u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    if number(92) != number(24) {
        return Err(io::Error::other("the header's page count is not valid"));
    }
    let page_size = match u16::from_be_bytes([header[16], header[17]]) {
        1 => 65536,
        size => u64::from(size),
    };
    Ok(page_size * u64::from(number(28)))
}

#[wasm_bindgen]
impl Import {
    /// Starts importing a gzipped database file as `name`. The inflated size comes from the
    /// database's own header, so the file's size has no limit but the pool's. Nothing is visible
    /// under `name` until `finish`.
    pub async fn begin(name: String) -> Result<Import, JsError> {
        let target =
            Target { pool: pool().await?, name, head: Vec::new(), import: None, fault: None };
        let inflate = GzDecoder::new(BufWriter::with_capacity(WRITE_CHUNK, target));
        Ok(Import { inflate, hash: Sha256::new() })
    }

    /// Takes the gzip's next bytes. Bytes that do not inflate, or a header that does not say how
    /// big the database is, throw as a damaged snapshot.
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), JsError> {
        self.hash.update(bytes);
        let written = self.inflate.write_all(bytes);
        written.map_err(|error| failure(self.inflate.get_mut().get_mut(), error))
    }

    /// Publishes the file once the whole gzip has been written. With `sha256` (lowercase hex),
    /// the file is published only if the gzip written hashes to it; otherwise, or if the gzip is
    /// cut short, its checksum fails, or it held less than a header or less than the header said,
    /// it throws and the import is discarded.
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
        // Flushed, so nothing is left in the buffer. The import's own count, not gzip's length
        // check, which wraps at 4 GiB, says whether it is whole.
        let (target, _) = inflate.finish()?.into_parts();
        let Some(import) = target.import else {
            return Err(JsError::new(&format!("{DAMAGED}: it ends before a database's header")));
        };
        match import.finish() {
            Err(error @ OpfsSAHError::Transfer(_)) => {
                Err(JsError::new(&format!("{DAMAGED}: {error}")))
            }
            result => Ok(result?),
        }
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
