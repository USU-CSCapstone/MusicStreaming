//! The database: one writer and a pool of readers, each a thread owning its connection.
//!
//! All writes queue for the single writer, so they never contend, and change-feed sequence
//! numbers are assigned in commit order (design/database.md §5). Readers are read-only
//! connections that WAL lets run alongside the writer, each seeing a consistent snapshot.
//!
//! SQLite calls block, so they run on these threads rather than on the async runtime.
//! Callers pass a closure and await its result, or, on threads outside the runtime such as
//! the scanner's, wait for it.

// Read-side helpers here are used by the admin API as it is built and by
// the tests meanwhile.
#![allow(dead_code)]

pub mod catalog;
pub mod feed;
pub mod libraries;
pub mod migrations;
pub mod store;
#[cfg(test)]
mod tests_store;

use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use anyhow::{Context, bail};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use tokio::sync::oneshot;
use tracing::{error, info, warn};

pub use store::SqliteStore;

/// Work for a database thread. It sends its own result back to whoever queued it.
type Job = Box<dyn FnOnce(&mut Connection) + Send>;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// The job panicked, or the database has shut down.
    #[error("the database job did not complete")]
    Aborted,
}

pub struct Database {
    // Fields drop in declaration order: closing both queues first lets every thread finish its
    // loop, and dropping `_threads` then waits for them, so every connection is closed
    // before the data directory's lock is released.
    writer: mpsc::Sender<Job>,
    readers: mpsc::Sender<Job>,
    _threads: Threads,
}

impl Database {
    /// Opens or creates the database, brings its schema up to date, and starts its threads.
    pub fn open(path: &Path) -> anyhow::Result<Database> {
        let mut writer = Connection::open(path)
            .with_context(|| format!("cannot open the database {}", path.display()))?;
        configure(&writer)?;
        let mode: String = writer.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if mode != "wal" {
            bail!(
                "the database {} cannot use write-ahead logging (journal mode is {mode}); \
                 the data directory must be on a local filesystem",
                path.display()
            );
        }
        // Every commit reaches disk before it is acknowledged. A commit lost to power failure
        // would let the change feeds reuse sequence numbers clients have already seen.
        writer.execute_batch("PRAGMA synchronous = FULL")?;

        let schema_version = migrations::migrate(&mut writer, migrations::MIGRATIONS)?;
        // Gathers query-planner statistics where they are missing (design/database.md §4).
        writer.execute_batch("PRAGMA optimize = 0x10002")?;

        let reader_count = thread::available_parallelism()
            .map_or(4, NonZeroUsize::get)
            .max(2);
        let mut readers = Vec::with_capacity(reader_count);
        for _ in 0..reader_count {
            let reader = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .with_context(|| format!("cannot open the database {}", path.display()))?;
            configure(&reader)?;
            readers.push(reader);
        }

        let (writer_queue, writer_jobs) = mpsc::channel::<Job>();
        let (reader_queue, reader_jobs) = mpsc::channel::<Job>();
        // Readers take turns receiving from one queue, so a job goes to whichever is free.
        let reader_jobs = Arc::new(Mutex::new(reader_jobs));

        let mut threads = Vec::with_capacity(reader_count + 1);
        threads.push(spawn("db-writer", move || {
            let mut conn = writer;
            for job in writer_jobs {
                run(job, &mut conn);
            }
            if let Err(error) = conn.execute_batch("PRAGMA optimize") {
                warn!(%error, "cannot optimize the database on shutdown");
            }
        })?);
        for (index, mut conn) in readers.into_iter().enumerate() {
            let jobs = Arc::clone(&reader_jobs);
            threads.push(spawn(&format!("db-reader-{index}"), move || {
                loop {
                    // The lock is held only while waiting, and released before the job runs.
                    let job = jobs.lock().expect("reader queue lock poisoned").recv();
                    let Ok(job) = job else { break };
                    run(job, &mut conn);
                }
            })?);
        }

        info!(
            path = %path.display(),
            schema_version,
            readers = reader_count,
            "database ready"
        );
        Ok(Database {
            writer: writer_queue,
            readers: reader_queue,
            _threads: Threads(threads),
        })
    }

    /// Runs `f` on a reader. Every query inside it sees the same snapshot of the database.
    pub async fn read<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        queue(&self.readers, in_snapshot(f))?
            .await
            .map_err(|_| DbError::Aborted)?
    }

    /// [`read`](Self::read), for threads outside the async runtime. Panics inside it.
    pub fn read_blocking<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        queue(&self.readers, in_snapshot(f))?
            .blocking_recv()
            .map_err(|_| DbError::Aborted)?
    }

    /// Runs `f` on the writer in one transaction, committed if it returns `Ok` and rolled back
    /// otherwise.
    pub async fn write<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Transaction) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        queue(&self.writer, in_transaction(f))?
            .await
            .map_err(|_| DbError::Aborted)?
    }

    /// [`write`](Self::write), for threads outside the async runtime. Panics inside it.
    pub fn write_blocking<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Transaction) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        queue(&self.writer, in_transaction(f))?
            .blocking_recv()
            .map_err(|_| DbError::Aborted)?
    }
}

/// Runs `f` in a read transaction, so all of its queries see one snapshot.
fn in_snapshot<T, F>(f: F) -> impl FnOnce(&mut Connection) -> Result<T, DbError>
where
    F: FnOnce(&Connection) -> rusqlite::Result<T>,
{
    move |conn| {
        let snapshot = conn.transaction()?;
        Ok(f(&snapshot)?)
    }
}

/// Runs `f` in a write transaction, committed if it returns `Ok` and rolled back otherwise.
fn in_transaction<T, F>(f: F) -> impl FnOnce(&mut Connection) -> Result<T, DbError>
where
    F: FnOnce(&Transaction) -> rusqlite::Result<T>,
{
    move |conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }
}

/// Settings every connection needs
fn configure(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
        .context("cannot configure a database connection")
}

/// Queues `work` and returns where its result will arrive.
fn queue<T, W>(
    queue: &mpsc::Sender<Job>,
    work: W,
) -> Result<oneshot::Receiver<Result<T, DbError>>, DbError>
where
    W: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    T: Send + 'static,
{
    let (reply, result) = oneshot::channel();
    let job: Job = Box::new(move |conn| {
        let _ = reply.send(work(conn));
    });
    queue.send(job).map_err(|_| DbError::Aborted)?;
    Ok(result)
}

/// Runs a job, keeping the thread alive if it panics.
fn run(job: Job, conn: &mut Connection) {
    if panic::catch_unwind(AssertUnwindSafe(|| job(conn))).is_err() {
        error!("a database job panicked");
    }
}

/// Spawns a thread and returns its handle.
fn spawn(name: &str, body: impl FnOnce() + Send + 'static) -> anyhow::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .with_context(|| format!("cannot start the {name} thread"))
}

/// A random positive 63-bit id (`design/database.md` §1). Callers retry on
/// a unique-constraint collision.
pub fn new_id() -> i64 {
    loop {
        let v = rand::random::<u64>() & 0x7FFF_FFFF_FFFF_FFFF;
        if v != 0 {
            return v as i64;
        }
    }
}

pub fn now_ms() -> i64 {
    jewelcase_scanner::now_ms() as i64
}

/// Waits for the database threads when dropped.
struct Threads(Vec<JoinHandle<()>>);

impl Drop for Threads {
    fn drop(&mut self) {
        for thread in self.0.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> (tempfile::TempDir, Database) {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("jewelcase.db")).unwrap();
        (temp, database)
    }

    const INSERT_LIBRARY: &str =
        "INSERT INTO libraries (id, name, created_at, updated_at) VALUES (?1, 'Music', 0, 0)";

    #[tokio::test]
    async fn reads_see_committed_writes() {
        let (_temp, db) = open();
        db.write(|tx| tx.execute(INSERT_LIBRARY, [1]))
            .await
            .unwrap();
        let name: String = db
            .read(|conn| {
                conn.query_row("SELECT name FROM libraries WHERE id = 1", [], |row| {
                    row.get(0)
                })
            })
            .await
            .unwrap();
        assert_eq!(name, "Music");
    }

    #[tokio::test]
    async fn a_failed_write_rolls_back() {
        let (_temp, db) = open();
        let result = db
            .write(|tx| {
                tx.execute(INSERT_LIBRARY, [1])?;
                tx.execute(INSERT_LIBRARY, [1]) // duplicate id
            })
            .await;
        assert!(matches!(result, Err(DbError::Sqlite(_))));
        let count: i64 = db
            .read(|conn| conn.query_row("SELECT count(*) FROM libraries", [], |row| row.get(0)))
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn readers_cannot_write() {
        let (_temp, db) = open();
        let result = db.read(|conn| conn.execute(INSERT_LIBRARY, [1])).await;
        assert!(matches!(result, Err(DbError::Sqlite(_))));
    }

    #[tokio::test]
    async fn foreign_keys_are_enforced() {
        let (_temp, db) = open();
        let result = db
            .write(|tx| {
                tx.execute(
                    "INSERT INTO library_roots (id, library_id, path, created_at) VALUES (1, 404, '/music', 0)",
                    [],
                )
            })
            .await;
        assert!(matches!(result, Err(DbError::Sqlite(_))));
    }

    #[tokio::test]
    async fn a_panicking_job_does_not_take_down_its_thread() {
        let (_temp, db) = open();
        let result = db
            .write(|_| -> rusqlite::Result<()> { panic!("boom") })
            .await;
        assert!(matches!(result, Err(DbError::Aborted)));
        db.write(|tx| tx.execute(INSERT_LIBRARY, [1]))
            .await
            .unwrap();
    }

    #[test]
    fn blocking_calls_work_outside_the_runtime() {
        let (_temp, db) = open();
        db.write_blocking(|tx| tx.execute(INSERT_LIBRARY, [1]))
            .unwrap();
        let count: i64 = db
            .read_blocking(|conn| {
                conn.query_row("SELECT count(*) FROM libraries", [], |row| row.get(0))
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn ids_are_positive_and_distinct() {
        let a = new_id();
        let b = new_id();
        assert!(a > 0 && b > 0);
        assert_ne!(a, b);
    }

    #[test]
    fn reopening_keeps_the_data() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("jewelcase.db");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        {
            let db = Database::open(&path).unwrap();
            runtime
                .block_on(db.write(|tx| tx.execute(INSERT_LIBRARY, [1])))
                .unwrap();
        }
        let db = Database::open(&path).unwrap();
        let count: i64 = runtime
            .block_on(db.read(|conn| {
                conn.query_row("SELECT count(*) FROM libraries", [], |row| row.get(0))
            }))
            .unwrap();
        assert_eq!(count, 1);
    }
}
