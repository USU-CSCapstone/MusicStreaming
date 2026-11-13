//! `getLibraryCatalog` (`api/openapi.yaml`): a library's catalog as a gzipped device-schema SQLite
//! file, built in the background and kept under `cache/catalog/<library>/` (`design/general.md`
//! §3).
//!
//! A file is `<version>-<epoch>-<position>-<size>-<sha256>.gz`, written by one build and named for
//! the device schema version it holds, the feed position it is current to and the epoch of the run
//! that gave it, the size of the database it inflates to, and its SHA-256, which is its ETag. Only
//! the gzip is kept: it is served as it is, so a range of it is a range of what the client stores,
//! and an interrupted download resumes. A file of another schema version is never served, so a
//! release that changes the schema builds afresh, and neither is one whose position this database
//! cannot go on from, such as a file newer than a restored backup. The newest is served; once the
//! feed holds more changes since it than a tenth of the library's entities, a rebuild starts and
//! the old file is served meanwhile. A build works inside a `building-*` folder of its own, so a
//! file is in place only whole.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use flate2::Compression;
use flate2::write::GzEncoder;
use jewelcase_catalog::{SCHEMA_VERSION, create_schema, snapshot};
use jewelcase_core::Recover;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use tower_http::services::ServeFile;

use super::scope::Scope;
use super::{Code, Problem};
use crate::db::Database;
use crate::db::feed::{self, Feed};

/// The page size of a snapshot, fixed so a file is the same shape whichever SQLite built it.
const PAGE_SIZE: u32 = 4096;

/// What `Retry-After` says while the first build runs, in seconds.
const RETRY_AFTER: i64 = 30;

/// How long after a build fails the next request for that library's file waits before building
/// again, so a failure that persists, such as a full disk, is not retried at every request.
const RETRY_COOLDOWN: Duration = Duration::from_secs(300);

/// The folder name a build works in.
const BUILDING: &str = "building-";

/// Where the library catalog files are kept and which libraries are being built.
pub struct Snapshots {
    dir: PathBuf,
    /// The server's database, which a build reads.
    database: PathBuf,
    /// The libraries with a build running, so no library is built twice at once.
    building: Mutex<HashSet<i64>>,
    /// When each library's last build failed, until one succeeds.
    failed: Mutex<HashMap<i64, Instant>>,
}

/// A library's build, registered in [`Snapshots::building`] until dropped, even if it panics.
struct Build<'a> {
    snapshots: &'a Snapshots,
    library: i64,
}

impl Drop for Build<'_> {
    /// Lets the library be built again.
    fn drop(&mut self) {
        self.snapshots.building.lock().recover().remove(&self.library);
    }
}

/// A finished file, as its name says.
struct Current {
    epoch: i64,
    position: i64,
    /// The bytes of the database the gzip inflates to.
    size: u64,
    sha256: String,
    path: PathBuf,
}

impl Snapshots {
    /// Keeps files in `dir`, built from the server database at `database`.
    pub fn new(dir: PathBuf, database: PathBuf) -> Snapshots {
        Snapshots { dir, database, building: Mutex::default(), failed: Mutex::default() }
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

    /// Starts a build of `library`'s file in run `epoch` in the background, unless one is
    /// running or the last one failed less than [`RETRY_COOLDOWN`] ago. It runs on a thread of
    /// its own, at lowered priority, since a pooled thread would keep the lowered priority for
    /// later tasks.
    fn start(self: &Arc<Self>, library: i64, epoch: i64) {
        let cooling = self
            .failed
            .lock()
            .recover()
            .get(&library)
            .is_some_and(|at| at.elapsed() < RETRY_COOLDOWN);
        if cooling || !self.building.lock().recover().insert(library) {
            return;
        }
        let snapshots = self.clone();
        let spawned = std::thread::Builder::new().name("catalog-build".into()).spawn(move || {
            let _build = Build { snapshots: &snapshots, library };
            jewelcase_ffmpeg::lower_thread_priority();
            let dir = snapshots.dir.join(library.to_string());
            match build(&snapshots.database, &dir, library, epoch, quick_check) {
                Ok(()) => {
                    snapshots.failed.lock().recover().remove(&library);
                }
                Err(error) => {
                    tracing::warn!(%error, library, "cannot build the catalog file");
                    snapshots.failed.lock().recover().insert(library, Instant::now());
                }
            }
        });
        if let Err(error) = spawned {
            tracing::warn!(%error, library, "cannot start the catalog build");
            self.building.lock().recover().remove(&library);
        }
    }
}

/// The newest finished file of the current schema version in a library's folder, if there is
/// one.
fn newest(dir: &Path) -> io::Result<Option<Current>> {
    let entries = match fs::read_dir(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        entries => entries?,
    };
    let mut newest: Option<Current> = None;
    for entry in entries {
        let path = entry?.path();
        let parsed = path.file_name().and_then(|name| {
            let mut parts = name.to_str()?.strip_suffix(".gz")?.splitn(5, '-');
            let version = parts.next()?.parse::<i32>().ok()?;
            let epoch = parts.next()?.parse::<i64>().ok()?;
            let position = parts.next()?.parse::<i64>().ok()?;
            let size = parts.next()?.parse::<u64>().ok()?;
            Some((version, epoch, position, size, parts.next()?.to_owned()))
        });
        if let Some((version, epoch, position, size, sha256)) = parsed
            && version == SCHEMA_VERSION
            && newest.as_ref().is_none_or(|n| position > n.position)
        {
            newest = Some(Current { epoch, position, size, sha256, path });
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

/// Builds `library`'s file in `dir` from the server database at `database` in run `epoch`,
/// replacing the older ones: the device rows and the feed position read in one snapshot, checked
/// by `verify`, hashed, and gzipped, keeping only the gzip. The file served is the database as
/// built, closed before it is read, not a compacted copy: a copy is 3.5% smaller on the wire but
/// is another write of the whole file. Nothing is left in `dir` if any step fails.
fn build(
    database: &Path,
    dir: &Path,
    library: i64,
    epoch: i64,
    verify: fn(&Connection) -> rusqlite::Result<()>,
) -> anyhow::Result<()> {
    fs::create_dir_all(dir)?;
    let work = tempfile::Builder::new().prefix(BUILDING).tempdir_in(dir)?;
    let served = work.path().join("served.db");
    let mut conn = Connection::open(&served)?;
    conn.execute_batch(&format!("PRAGMA page_size = {PAGE_SIZE}; PRAGMA journal_mode = DELETE"))?;
    create_schema(&conn)?;
    // Read-only, and attached before the transaction `copy` makes, which SQLite requires.
    conn.execute("ATTACH DATABASE ?1 AS server", [read_only_uri(database)])?;
    let position = snapshot::copy(&mut conn, library)?;
    conn.execute_batch("DETACH DATABASE server")?;
    conn.close().map_err(|(_, error)| error)?;
    verify(&Connection::open_with_flags(&served, OpenFlags::SQLITE_OPEN_READ_ONLY)?)?;

    let size = fs::metadata(&served)?.len();
    let zipped = work.path().join("served.db.gz");
    let sha256 = compress(&served, &zipped)?;
    let name = format!("{SCHEMA_VERSION}-{epoch}-{position}-{size}-{sha256}.gz");
    fs::rename(zipped, dir.join(&name))?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.file_name() != name.as_str() {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Gzips `from` into `to` and returns the SHA-256 of `to`, the bytes served, as lowercase hex.
fn compress(from: &Path, to: &Path) -> io::Result<String> {
    let mut output = GzEncoder::new(File::create(to)?, Compression::default());
    io::copy(&mut File::open(from)?, &mut output)?;
    output.finish()?.sync_all()?;
    let mut hash = Sha256::new();
    io::copy(&mut File::open(to)?, &mut hash)?;
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

/// The spec's `getLibraryCatalog` and `headLibraryCatalog` (axum answers a `HEAD` with this
/// handler and drops the body, so it passes the same access check and starts any build a `GET`
/// would): the library's newest file, whole or the range asked for, with its ETag, feed position
/// and epoch, device schema version and inflated size. A file the feed has moved too far past is
/// rebuilt in the background. With no file yet, or none this database can go on from,
/// `503 catalog_building` and a build.
pub async fn get(
    State(db): State<Arc<Database>>,
    State(snapshots): State<Arc<Snapshots>>,
    Scope { library, .. }: Scope,
    request: Request,
) -> Result<Response, Problem> {
    let dir = snapshots.dir.join(library.to_string());
    let current = tokio::task::spawn_blocking(move || newest(&dir)).await.map_err(internal)?;
    let current = current.map_err(internal)?;
    let cursor = current.as_ref().map_or((None, 0), |c| (Some(c.epoch), c.position));
    let (follows, entities, changed): (bool, i64, i64) = db
        .read(move |conn| {
            let (epoch, after) = cursor;
            let follows = feed::continues(conn, Feed::Library(library), epoch, after)?;
            let (entities, changed) = conn.query_row(
                "SELECT track_count + album_count + artist_count, \
                 (SELECT count(*) FROM library_changes WHERE library_id = ?1 AND position > ?2) \
                 FROM libraries WHERE id = ?1",
                [library, after],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok((follows, entities, changed))
        })
        .await?;
    // A file this database cannot go on from is of another timeline, such as a restored backup's.
    let current = current.filter(|_| follows);
    if current.is_none() || changed > entities / 10 {
        snapshots.start(library, db.epoch());
    }
    let Some(current) = current else {
        return Err(Problem::catalog_building(RETRY_AFTER));
    };
    serve(snapshots.dir.join(library.to_string()), current, request).await
}

/// Serves `chosen` with the catalog's headers. A rebuild that finished since `chosen` was picked
/// has deleted it, so then the newest file in `dir`, its replacement, is served instead.
async fn serve(dir: PathBuf, chosen: Current, request: Request) -> Result<Response, Problem> {
    let (parts, _) = request.into_parts();
    let open = |current: &Current| {
        let request = Request::from_parts(conditions(&parts, &current.sha256), Body::empty());
        ServeFile::new(&current.path).try_call(request)
    };
    let (Current { epoch, position, size, sha256, .. }, served) = match open(&chosen).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let current = tokio::task::spawn_blocking(move || newest(&dir));
            let current = current.await.map_err(internal)?.map_err(internal)?;
            let current = current.ok_or_else(|| Problem::catalog_building(RETRY_AFTER))?;
            let served = open(&current).await;
            (current, served)
        }
        served => (chosen, served),
    };
    let mut response = served.map_err(internal)?.map(Body::new);
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/gzip"));
    // Not even the browser's HTTP cache keeps a copy, so none outlives a sign-out.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("x-jewelcase-position", HeaderValue::from(position));
    headers.insert("x-jewelcase-epoch", HeaderValue::from(epoch));
    headers.insert("x-jewelcase-schema", HeaderValue::from(SCHEMA_VERSION));
    headers.insert("x-jewelcase-database-size", HeaderValue::from(size));
    if let Ok(etag) = HeaderValue::try_from(format!("\"{sha256}\"")) {
        headers.insert(header::ETAG, etag);
    }
    Ok(response)
}

/// The request to hand the file server for a file whose ETag is `sha256`, which this module
/// gives rather than the file server: `Range` is kept only if any `If-Range` names this file.
fn conditions(parts: &Parts, sha256: &str) -> Parts {
    let mut parts = parts.clone();
    let headers = &mut parts.headers;
    if let Some(validator) = headers.remove(header::IF_RANGE)
        && validator.as_bytes() != format!("\"{sha256}\"").as_bytes()
    {
        headers.remove(header::RANGE);
    }
    parts
}

/// A `500 internal`, with the cause logged and kept from the client: it can name a path.
fn internal(error: impl std::fmt::Display) -> Problem {
    tracing::error!(%error, "cannot serve the catalog file");
    Problem::new(Code::Internal)
}

#[cfg(test)]
mod tests;
