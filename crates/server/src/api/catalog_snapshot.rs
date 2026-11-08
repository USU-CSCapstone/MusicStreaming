//! `getLibraryCatalog` (`api/openapi.yaml`): a library's catalog as a device-schema SQLite file,
//! built in the background and kept under `cache/catalog/<library>/` (`design/general.md` §3).
//!
//! A file is `<seq>-<sha256>.db` and its gzip `<seq>-<sha256>.db.gz`, written by one build and
//! named for the feed position it is current to and for the hash of the database itself, which is
//! its ETag. The newest is served; once the feed holds more changes since it than a tenth of the
//! library's entities, a rebuild starts and the old file is served meanwhile. A build works inside
//! a `building-*` folder of its own, so a file is in place only whole.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use flate2::Compression;
use flate2::write::GzEncoder;
use jewelcase_catalog::{create_schema, snapshot};
use jewelcase_core::Recover;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use tower_http::services::ServeFile;

use super::scope::Scope;
use super::{Code, Problem};
use crate::db::Database;

/// The page size of a snapshot, fixed so a file is the same shape whichever SQLite built it.
const PAGE_SIZE: u32 = 4096;

/// What `Retry-After` says while the first build runs, in seconds.
const RETRY_AFTER: i64 = 30;

/// The folder name a build works in.
const BUILDING: &str = "building-";

/// Where the library catalog files are kept and which libraries are being built.
pub struct Snapshots {
    dir: PathBuf,
    /// The server's database, which a build reads.
    database: PathBuf,
    /// The libraries with a build running, so no library is built twice at once.
    building: Mutex<HashSet<i64>>,
}

/// A library's build, registered in [`Snapshots::building`] until dropped, even if it panics.
struct Build<'a> {
    snapshots: &'a Snapshots,
    library: i64,
}

impl Drop for Build<'_> {
    fn drop(&mut self) {
        self.snapshots.building.lock().recover().remove(&self.library);
    }
}

/// A finished file, as its name says.
struct Current {
    seq: i64,
    sha256: String,
    path: PathBuf,
}

impl Snapshots {
    /// Keeps files in `dir`, built from the server database at `database`.
    pub fn new(dir: PathBuf, database: PathBuf) -> Snapshots {
        Snapshots { dir, database, building: Mutex::default() }
    }

    /// Deletes the files of libraries that are gone and the leftovers of builds cut short. Call
    /// it before the server takes requests, since it would delete a build's work in progress.
    pub async fn sweep(&self, db: &Database) -> io::Result<()> {
        let libraries: HashSet<i64> = db
            .read(|conn| {
                conn.prepare("SELECT id FROM libraries")?.query_map([], |r| r.get(0))?.collect()
            })
            .await
            .map_err(io::Error::other)?;
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || {
            let entries = match fs::read_dir(&dir) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                entries => entries?,
            };
            for entry in entries {
                let path = entry?.path();
                let id = path.file_name().and_then(|name| name.to_str()?.parse::<i64>().ok());
                if id.is_some_and(|id| libraries.contains(&id)) {
                    for work in fs::read_dir(&path)? {
                        let work = work?.path();
                        if work.is_dir() {
                            fs::remove_dir_all(work)?;
                        }
                    }
                } else if path.is_dir() {
                    fs::remove_dir_all(&path)?;
                } else {
                    fs::remove_file(&path)?;
                }
            }
            Ok(())
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Starts a build of `library`'s file in the background, unless one is running.
    fn start(self: &Arc<Self>, library: i64) {
        if !self.building.lock().recover().insert(library) {
            return;
        }
        let snapshots = self.clone();
        tokio::task::spawn_blocking(move || {
            let _build = Build { snapshots: &snapshots, library };
            let dir = snapshots.dir.join(library.to_string());
            if let Err(error) = build(&snapshots.database, &dir, library, quick_check) {
                tracing::warn!(%error, library, "cannot build the catalog file");
            }
        });
    }
}

/// The newest finished file in a library's folder, if there is one.
fn newest(dir: &Path) -> io::Result<Option<Current>> {
    let entries = match fs::read_dir(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        entries => entries?,
    };
    let mut newest: Option<Current> = None;
    for entry in entries {
        let path = entry?.path();
        let parsed = path.file_name().and_then(|name| {
            let (seq, sha256) = name.to_str()?.strip_suffix(".db")?.split_once('-')?;
            Some((seq.parse::<i64>().ok()?, sha256.to_owned()))
        });
        if let Some((seq, sha256)) = parsed
            && newest.as_ref().is_none_or(|n| seq > n.seq)
        {
            newest = Some(Current { seq, sha256, path });
        }
    }
    Ok(newest)
}

/// SQLite's `quick_check` on the file, which fails unless it reports `ok`.
fn quick_check(conn: &Connection) -> rusqlite::Result<()> {
    let result: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if result == "ok" {
        Ok(())
    } else {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            Some(result),
        ))
    }
}

/// Builds `library`'s file in `dir` from the server database at `database`, replacing the older
/// ones: the device rows and the feed position read in one snapshot, compacted with
/// `VACUUM INTO`, checked by `verify`, hashed, and gzipped. Nothing is left in `dir` if any step
/// fails.
fn build(
    database: &Path,
    dir: &Path,
    library: i64,
    verify: fn(&Connection) -> rusqlite::Result<()>,
) -> anyhow::Result<()> {
    fs::create_dir_all(dir)?;
    let work = tempfile::Builder::new().prefix(BUILDING).tempdir_in(dir)?;
    let mut conn = Connection::open(work.path().join("copy.db"))?;
    conn.execute_batch(&format!("PRAGMA page_size = {PAGE_SIZE}; PRAGMA journal_mode = DELETE"))?;
    create_schema(&conn)?;
    // Read-only, and attached before the transaction `copy` makes, which SQLite requires.
    conn.execute("ATTACH DATABASE ?1 AS server", [read_only_uri(database)])?;
    let seq = snapshot::copy(&mut conn, library)?;
    conn.execute_batch("DETACH DATABASE server")?;
    let served = work.path().join("served.db");
    conn.execute("VACUUM INTO ?1", [served.to_str()])?;
    drop(conn);
    fs::remove_file(work.path().join("copy.db"))?;
    verify(&Connection::open_with_flags(&served, OpenFlags::SQLITE_OPEN_READ_ONLY)?)?;

    let zipped = work.path().join("served.db.gz");
    let sha256 = compress(&served, &zipped)?;
    File::open(&served)?.sync_all()?;
    let name = format!("{seq}-{sha256}.db");
    // The file that names the snapshot is the last in place.
    fs::rename(zipped, dir.join(format!("{name}.gz")))?;
    fs::rename(served, dir.join(&name))?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && !entry.file_name().to_string_lossy().starts_with(&name) {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Gzips `from` into `to` and returns the SHA-256 of `from` as lowercase hex.
fn compress(from: &Path, to: &Path) -> io::Result<String> {
    let mut input = File::open(from)?;
    let mut output = GzEncoder::new(File::create(to)?, Compression::default());
    let mut hash = Sha256::new();
    let mut chunk = vec![0; 1 << 16];
    loop {
        let read = input.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        hash.update(&chunk[..read]);
        output.write_all(&chunk[..read])?;
    }
    output.finish()?.sync_all()?;
    Ok(format!("{:x}", hash.finalize()))
}

/// `path` as a SQLite URI that opens the file read-only, escaping what a URI reserves.
fn read_only_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'/' | b'-' | b'_' | b'.' | b'~' => uri.push(char::from(*byte)),
            b if b.is_ascii_alphanumeric() => uri.push(char::from(*b)),
            b => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri + "?mode=ro"
}

/// The spec's `getLibraryCatalog`: the library's newest file, gzipped if the client accepts it,
/// with its ETag and feed position. A file the feed has moved too far past is rebuilt in the
/// background. With no file yet, `503 catalog_building` and a build.
pub async fn get(
    State(db): State<Arc<Database>>,
    State(snapshots): State<Arc<Snapshots>>,
    Scope { library, .. }: Scope,
    request: Request,
) -> Result<Response, Problem> {
    let dir = snapshots.dir.join(library.to_string());
    let current = tokio::task::spawn_blocking(move || newest(&dir)).await.map_err(internal)?;
    let current = current.map_err(internal)?;
    let after = current.as_ref().map_or(0, |c| c.seq);
    let (head, entities, changed): (i64, i64, i64) = db
        .read(move |conn| {
            conn.query_row(
                "SELECT (SELECT coalesce(max(seq), 0) FROM library_changes WHERE library_id = ?1), \
                 track_count + album_count + artist_count, \
                 (SELECT count(*) FROM library_changes WHERE library_id = ?1 AND seq > ?2) \
                 FROM libraries WHERE id = ?1",
                [library, after],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
        })
        .await?;
    // A file past the library's head is from another timeline, such as a restored backup.
    let current = current.filter(|c| c.seq <= head);
    if current.is_none() || changed > entities / 10 {
        snapshots.start(library);
    }
    let Some(Current { seq, sha256, path }) = current else {
        return Err(Problem::catalog_building(RETRY_AFTER));
    };
    let mut response = ServeFile::new(path)
        .precompressed_gzip()
        .try_call(request)
        .await
        .map_err(internal)?
        .map(Body::new);
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/vnd.sqlite3"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    headers.insert("x-jewelcase-seq", HeaderValue::from(seq));
    if let Ok(etag) = HeaderValue::try_from(format!("\"{sha256}\"")) {
        headers.insert(header::ETAG, etag);
    }
    Ok(response)
}

/// A `500 internal`, with the cause logged and kept from the client: it can name a path.
fn internal(error: impl std::fmt::Display) -> Problem {
    tracing::error!(%error, "cannot serve the catalog file");
    Problem::new(Code::Internal)
}

#[cfg(test)]
mod tests;
