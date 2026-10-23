//! The database: one writer and a pool of readers, each a thread owning its connection.
//!
//! All writes queue for the single writer, so they never contend, and change-feed sequence
//! numbers are assigned in commit order (design/database.md §5). Readers are read-only
//! connections that WAL lets run alongside the writer, each seeing a consistent snapshot.
//!
//! SQLite calls block, so they run on these threads rather than on the async runtime.
//! Callers pass a closure and await its result, or, on threads outside the runtime such as
//! the scanner's, wait for it.

mod by_ids;
pub mod catalog;
mod columns;
pub mod feed;
pub mod libraries;
pub mod migrations;
mod new_id;
pub mod plays;
mod reach;
pub mod retention;
pub mod store;
#[cfg(test)]
pub mod testing;
#[cfg(test)]
mod tests_store;
pub mod waveform;

use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, bail};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use tokio::sync::oneshot;
use tracing::{error, info, warn};

pub use by_ids::by_ids;
pub use columns::{Json, Text};
pub use new_id::new_id;
pub(crate) use reach::reaches;
use retention::Sweep;
pub use store::SqliteStore;

use crate::lock::Recover;

/// How often the writer sweeps away what is no longer kept (`design/database.md` §9), then
/// gathers query-planner statistics, which the planner needs to choose indexes (§4).
const SWEEP_EVERY: Duration = Duration::from_secs(3600);

/// The fewest tracks, across every library, for which the writer gathers query-planner
/// statistics. Statistics from a smaller library describe tables of a few rows, which lead the
/// planner to scan a table that has since grown rather than look its rows up, until
/// `PRAGMA optimize` notices that it has grown tenfold. Without statistics the planner takes
/// every table to be large (`design/database.md` §4).
const STATISTICS_FROM_TRACKS: i64 = 1_000;

/// Gathers query-planner statistics where they are missing (0x10000 checks every table, 0x2
/// analyzes), within SQLite's analysis limit (0x10), so even a large database opens quickly.
const OPTIMIZE_ON_OPEN: &str = "PRAGMA optimize = 0x10012";

/// How long after the database opens the writer first sweeps, so a server restarted more often
/// than [`SWEEP_EVERY`] still sweeps, without adding to its startup.
const FIRST_SWEEP: Duration = Duration::from_secs(300);

/// How much of the database file each reader maps into memory, in bytes, so its reads come from
/// the operating system's page cache, which every reader shares, rather than being copied into
/// a small cache of its own (`design/database.md` §7). 1 GiB is about the database of a
/// 500k-track library.
const READER_MMAP_SIZE: i64 = 1 << 30;

/// The size, in bytes, the writer cuts the write-ahead log back to when it starts the log over
/// after a checkpoint, so the disk a cold scan's log took is given back (`design/database.md` §7).
const JOURNAL_SIZE_LIMIT: i64 = 64 << 20;

/// How many prepared statements each connection keeps for `prepare_cached` to reuse, least
/// recently used dropped first: more than the store and the catalog recompute run per batch, so a
/// scan prepares each only once (`design/database.md` §7).
const STATEMENT_CACHE: usize = 128;

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
        Database::open_sweeping(path, FIRST_SWEEP)
    }

    /// [`open`](Self::open), with the writer's first sweep `first_sweep` after it opens.
    fn open_sweeping(path: &Path, first_sweep: Duration) -> anyhow::Result<Database> {
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
        // Only the writer writes the log, so only its limit takes effect.
        writer.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT)?;

        let schema_version = migrations::migrate(&mut writer, migrations::MIGRATIONS)?;
        // Once the libraries are large enough to describe (design/database.md §4).
        gather_statistics(&writer, OPTIMIZE_ON_OPEN)?;

        let reader_count = thread::available_parallelism().map_or(4, NonZeroUsize::get).max(2);
        let mut readers = Vec::with_capacity(reader_count);
        for _ in 0..reader_count {
            let reader = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .with_context(|| format!("cannot open the database {}", path.display()))?;
            configure(&reader)?;
            reader.pragma_update(None, "mmap_size", READER_MMAP_SIZE)?;
            readers.push(reader);
        }

        let (writer_queue, writer_jobs) = mpsc::channel::<Job>();
        let (reader_queue, reader_jobs) = mpsc::channel::<Job>();
        // Readers take turns receiving from one queue, so a job goes to whichever is free.
        let reader_jobs = Arc::new(Mutex::new(reader_jobs));

        let mut threads = Vec::with_capacity(reader_count + 1);
        threads.push(spawn("db-writer", move || {
            let mut conn = writer;
            let mut next_sweep = Instant::now() + first_sweep;
            loop {
                match writer_jobs.recv_timeout(next_sweep.saturating_duration_since(Instant::now()))
                {
                    Ok(job) => run(job, &mut conn),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                // On the writer between jobs, so it never contends with a write for the lock.
                if Instant::now() >= next_sweep {
                    sweep(&mut conn, &writer_jobs, Sweep::new(now_ms(), retention::STEP_TIME));
                    optimize(&conn);
                    next_sweep = Instant::now() + SWEEP_EVERY;
                }
            }
            optimize(&conn);
        })?);
        for (index, mut conn) in readers.into_iter().enumerate() {
            let jobs = Arc::clone(&reader_jobs);
            threads.push(spawn(&format!("db-reader-{index}"), move || {
                loop {
                    // The lock is held only while waiting, and released before the job runs.
                    let job = jobs.lock().recover().recv();
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
        Ok(Database { writer: writer_queue, readers: reader_queue, _threads: Threads(threads) })
    }

    /// Runs `f` on a reader. Every query inside it sees the same snapshot of the database.
    pub async fn read<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.try_read(move |conn| f(conn).map_err(DbError::from)).await
    }

    /// [`read`](Self::read), for an `f` whose error is a type of the caller's own, such as a
    /// refusal, which is returned as it is. A database failure is converted into that type.
    pub async fn try_read<T, E, F>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&Connection) -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: From<DbError> + From<rusqlite::Error> + Send + 'static,
    {
        queue(&self.readers, in_snapshot(f))?.await.map_err(|_| DbError::Aborted)?
    }

    /// [`read`](Self::read), for threads outside the async runtime. Panics inside it.
    pub fn read_blocking<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let work = in_snapshot(move |conn| f(conn).map_err(DbError::from));
        queue(&self.readers, work)?.blocking_recv().map_err(|_| DbError::Aborted)?
    }

    /// Runs `f` on the writer in one transaction, committed if it returns `Ok` and rolled back
    /// otherwise.
    pub async fn write<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Transaction) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.try_write(move |tx| f(tx).map_err(DbError::from)).await
    }

    /// [`write`](Self::write), for an `f` whose error is a type of the caller's own, such as a
    /// refusal, which rolls the transaction back and is returned as it is. A database failure
    /// is converted into that type.
    pub async fn try_write<T, E, F>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&Transaction) -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: From<DbError> + From<rusqlite::Error> + Send + 'static,
    {
        queue(&self.writer, in_transaction(f))?.await.map_err(|_| DbError::Aborted)?
    }

    /// [`write`](Self::write), for threads outside the async runtime. Panics inside it.
    pub fn write_blocking<T, F>(&self, f: F) -> Result<T, DbError>
    where
        F: FnOnce(&Transaction) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let work = in_transaction(move |tx| f(tx).map_err(DbError::from));
        queue(&self.writer, work)?.blocking_recv().map_err(|_| DbError::Aborted)?
    }
}

/// Runs `f` in a read transaction, so all of its queries see one snapshot.
fn in_snapshot<T, E, F>(f: F) -> impl FnOnce(&mut Connection) -> Result<T, E>
where
    F: FnOnce(&Connection) -> Result<T, E>,
    E: From<rusqlite::Error>,
{
    move |conn| {
        let snapshot = conn.transaction()?;
        f(&snapshot)
    }
}

/// Runs `f` in a write transaction, committed if it returns `Ok` and rolled back otherwise.
fn in_transaction<T, E, F>(f: F) -> impl FnOnce(&mut Connection) -> Result<T, E>
where
    F: FnOnce(&Transaction) -> Result<T, E>,
    E: From<rusqlite::Error>,
{
    move |conn| {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }
}

/// Runs `sweep` on the writer a step at a time, and between steps the jobs queued meanwhile, so
/// none waits for more than a step. A step that fails or panics is rolled back, and the sweep
/// passes over the kind of row it was sweeping until the next sweep, so one row that cannot go
/// never holds up the others. The database shutting down ends the sweep.
fn sweep(conn: &mut Connection, jobs: &mpsc::Receiver<Job>, sweep: Sweep) {
    let mut rest = Some(sweep);
    while let Some(sweep) = rest {
        let step = panic::catch_unwind(AssertUnwindSafe(|| {
            in_transaction(move |tx| sweep.step(tx))(conn)
        }));
        rest = match step {
            Ok(Ok(rest)) => rest,
            Ok(Err(error)) => {
                warn!(%error, "cannot sweep part of the database");
                sweep.next()
            }
            Err(_) => {
                error!("a database sweep panicked");
                sweep.next()
            }
        };
        loop {
            match jobs.try_recv() {
                Ok(job) => run(job, conn),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
    }
}

/// Gathers query-planner statistics through [`gather_statistics`] (`design/database.md` §4),
/// logging a failure rather than returning it.
fn optimize(conn: &Connection) {
    if let Err(error) = gather_statistics(conn, "PRAGMA optimize") {
        warn!(%error, "cannot optimize the database");
    }
}

/// Runs `optimize`, a `PRAGMA optimize` statement, when the libraries together hold at least
/// [`STATISTICS_FROM_TRACKS`] tracks, and does nothing otherwise.
fn gather_statistics(conn: &Connection, optimize: &str) -> rusqlite::Result<()> {
    let tracks: i64 =
        conn.query_row("SELECT ifnull(SUM(track_count), 0) FROM libraries", [], |r| r.get(0))?;
    if tracks >= STATISTICS_FROM_TRACKS {
        conn.execute_batch(optimize)?;
    }
    Ok(())
}

/// Settings every connection needs, and room in its statement cache for every statement the
/// scanner's writes and the busiest reads repeat ([`STATEMENT_CACHE`]).
fn configure(conn: &Connection) -> anyhow::Result<()> {
    conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
        .context("cannot configure a database connection")
}

/// Queues `work` and returns where its result will arrive.
fn queue<R, W>(queue: &mpsc::Sender<Job>, work: W) -> Result<oneshot::Receiver<R>, DbError>
where
    W: FnOnce(&mut Connection) -> R + Send + 'static,
    R: Send + 'static,
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

/// The current wall-clock time in milliseconds since the Unix epoch, as the schema keeps times.
pub fn now_ms() -> i64 {
    let since_epoch = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH);
    since_epoch.map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

/// Waits for the database threads when dropped.
struct Threads(Vec<JoinHandle<()>>);

impl Drop for Threads {
    /// Waits for each thread, logging one that panicked.
    fn drop(&mut self) {
        for thread in self.0.drain(..) {
            if thread.join().is_err() {
                error!("a database thread panicked");
            }
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
        "INSERT INTO libraries (id, name, created_at) VALUES (?1, 'Music', 0)";

    #[tokio::test]
    async fn reads_see_committed_writes() {
        let (_temp, db) = open();
        db.write(|tx| tx.execute(INSERT_LIBRARY, [1])).await.unwrap();
        let name: String = db
            .read(|conn| {
                conn.query_row("SELECT name FROM libraries WHERE id = 1", [], |row| row.get(0))
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

    /// An error of the caller's own, which `try_write` and `try_read` return as it is.
    #[derive(Debug, PartialEq)]
    enum Refusal {
        /// What the test's `f` returns.
        Refused,
        /// Any database failure.
        Failed,
    }

    impl From<DbError> for Refusal {
        /// Any database failure, as [`Refusal::Failed`].
        fn from(_: DbError) -> Refusal {
            Refusal::Failed
        }
    }

    impl From<rusqlite::Error> for Refusal {
        /// Any database error, as [`Refusal::Failed`].
        fn from(_: rusqlite::Error) -> Refusal {
            Refusal::Failed
        }
    }

    #[tokio::test]
    async fn a_refused_write_rolls_back_and_returns_the_refusal() {
        let (_temp, db) = open();
        let result = db
            .try_write(|tx| {
                tx.execute(INSERT_LIBRARY, [1])?;
                Err::<(), _>(Refusal::Refused)
            })
            .await;
        assert_eq!(result, Err(Refusal::Refused));
        let count: i64 = db
            .read(|conn| conn.query_row("SELECT count(*) FROM libraries", [], |row| row.get(0)))
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn a_refused_read_returns_the_refusal() {
        let (_temp, db) = open();
        let result = db.try_read(|_| Err::<(), _>(Refusal::Refused)).await;
        assert_eq!(result, Err(Refusal::Refused));
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

    /// Readers map the database into memory, and the writer, which writes the log, limits it.
    #[tokio::test]
    async fn readers_map_the_file_and_the_writer_limits_the_log() {
        let (_temp, db) = open();
        let pragma = |conn: &Connection, name: &str| {
            conn.pragma_query_value(None, name, |r| r.get::<_, i64>(0))
        };
        let mmap = db.read(move |conn| pragma(conn, "mmap_size")).await.unwrap();
        assert_eq!(mmap, READER_MMAP_SIZE);
        let limit = db.write(move |tx| pragma(tx, "journal_size_limit")).await.unwrap();
        assert_eq!(limit, JOURNAL_SIZE_LIMIT);
    }

    #[tokio::test]
    async fn a_panicking_job_does_not_take_down_its_thread() {
        let (_temp, db) = open();
        let result = db.write(|_| -> rusqlite::Result<()> { panic!("boom") }).await;
        assert!(matches!(result, Err(DbError::Aborted)));
        db.write(|tx| tx.execute(INSERT_LIBRARY, [1])).await.unwrap();
    }

    #[test]
    fn blocking_calls_work_outside_the_runtime() {
        let (_temp, db) = open();
        db.write_blocking(|tx| tx.execute(INSERT_LIBRARY, [1])).unwrap();
        let count: i64 = db
            .read_blocking(|conn| {
                conn.query_row("SELECT count(*) FROM libraries", [], |row| row.get(0))
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    /// A job queued as the writer sweeps runs after the step in progress, not after the whole
    /// sweep: one queued before it starts finds the first chunk of old scans gone and the rest
    /// still there.
    #[test]
    fn a_job_runs_between_the_steps_of_a_sweep() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("jewelcase.db");
        Database::open(&path)
            .unwrap()
            .write_blocking(|tx| {
                testing::library(tx, 1, "Music", "/music")?;
                for id in 1..=250 {
                    tx.execute(
                        "INSERT INTO scans (id, library_id, trigger, state, scopes, created_at, \
                         finished_at) VALUES (?1, 1, 'manual', 'completed', '[]', ?1, ?1)",
                        [id],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let mut conn = Connection::open(&path).unwrap();
        configure(&conn).unwrap();
        let scans = |conn: &Connection| {
            conn.query_row("SELECT count(*) FROM scans", [], |r| r.get::<_, i64>(0)).unwrap()
        };
        let (jobs, queued) = mpsc::channel::<Job>();
        let (seen, found) = mpsc::channel();
        jobs.send(Box::new(move |conn| seen.send(scans(conn)).unwrap())).unwrap();

        let now = retention::WINDOW_MS + 1000;
        sweep(&mut conn, &queued, Sweep::new(now, Duration::ZERO));

        let found = found.try_recv().unwrap();
        assert!(1 < found && found < 250, "{found} scans left");
        assert_eq!(scans(&conn), 1, "the last is kept");
    }

    /// The count that `sql`, a `count(*)` query, finds in the database at `path`.
    fn count(path: &Path, sql: &str) -> i64 {
        Connection::open(path).unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// The writer sweeps on its own once the first sweep's time comes after it opens.
    #[test]
    fn the_writer_sweeps_on_its_own() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("jewelcase.db");
        Database::open(&path)
            .unwrap()
            .write_blocking(|tx| {
                tx.execute(
                    "INSERT INTO login_attempts (attempted_at, username_attempted, origin, \
                     succeeded) VALUES (0, 'user', '192.0.2.1', 0)",
                    [],
                )
            })
            .unwrap();
        let _db = Database::open_sweeping(&path, Duration::ZERO).unwrap();
        let attempts = "SELECT count(*) FROM login_attempts";
        for _ in 0..500 {
            if count(&path, attempts) == 0 {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("the writer has not swept after 10 s");
    }

    /// A kind of row that cannot be swept is passed over, and the kinds after it are swept.
    #[test]
    fn a_sweep_passes_over_a_kind_that_fails() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("jewelcase.db");
        Database::open(&path)
            .unwrap()
            .write_blocking(|tx| {
                testing::library(tx, 1, "Music", "/music")?;
                testing::User::new(1, "owner").insert(tx)?;
                tx.execute_batch(
                    "INSERT INTO login_attempts (attempted_at, username_attempted, origin, \
                     succeeded) VALUES (0, 'user1', '192.0.2.1', 0); \
                     INSERT INTO recent_searches (user_id, library_id, query, folded, \
                     searched_at) VALUES (1, 1, 'q', 'q', 0); \
                     CREATE TRIGGER poison BEFORE DELETE ON login_attempts \
                     BEGIN SELECT RAISE(ABORT, 'poison'); END;",
                )
            })
            .unwrap();
        let mut conn = Connection::open(&path).unwrap();
        configure(&conn).unwrap();
        let (_jobs, queued) = mpsc::channel::<Job>();

        sweep(&mut conn, &queued, Sweep::new(retention::WINDOW_MS + 1000, Duration::ZERO));

        assert_eq!(count(&path, "SELECT count(*) FROM login_attempts"), 1);
        assert_eq!(count(&path, "SELECT count(*) FROM recent_searches"), 0);
    }

    #[test]
    fn reopening_keeps_the_data() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("jewelcase.db");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        {
            let db = Database::open(&path).unwrap();
            runtime.block_on(db.write(|tx| tx.execute(INSERT_LIBRARY, [1]))).unwrap();
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
