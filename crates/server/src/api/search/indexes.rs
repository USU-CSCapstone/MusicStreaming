//! Each library's search indexes, in memory: its names (tracks, albums, artists and tags) and its
//! lyrics, each tagged with a version of the library that the catalog's triggers bump whenever
//! what it indexes changes, and never for analysis, art or any other write
//! (`0007_catalog_counts.sql`): `names_version` when a track, album, artist or tag is added,
//! deleted or renamed, or a tag's track count changes, and `lyrics_version` when a track's lyrics
//! are added, changed or removed. So a lyric written rebuilds only the lyric index, and a rename
//! only the names one. Every library's indexes are built in the background as the server starts.
//! A search whose library has changed since starts a rebuild in the background and answers from
//! the index it has, so a search waits only while its library's first index is built. A deleted
//! library's indexes are dropped by the next search. The indexes are rebuildable data
//! (`design/database.md` §7).
//!
//! Rebuilds are spaced by how long the last one took, so a library whose names change
//! constantly, as they do during a scan, costs a tenth of a core at most, on any host and at any
//! size. A small library is rebuilt within moments of a change; a large one on slow hardware,
//! within seconds.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use jewelcase_catalog::search::documents;
use jewelcase_core::{Index, IndexBuilder, LyricIndex, LyricIndexBuilder, Recover};
use jewelcase_scanner::SyncedLine;
use rusqlite::Connection;

use super::tagged::TaggedSets;
use super::within::Containers;
use crate::api::page::library_number;
use crate::db::{self, Database, DbError};

/// A rebuild waits this many times as long as the last one took.
const REBUILD_SPACING: u32 = 10;

/// The tracks whose lyrics one read takes while a lyric index is built, so the build holds a
/// reader only briefly, and a batch of lyrics at a time, whatever the library's size. A test's
/// few tracks take several.
const LYRICS_BATCH: usize = if cfg!(test) { 2 } else { 1_000 };

/// Every library's search indexes, and the rows recent searches' tags and containers were read as.
#[derive(Default)]
pub struct Indexes {
    names: Libraries<Index>,
    lyrics: Libraries<LyricIndex>,
    pub tagged: TaggedSets,
    pub containers: Containers,
}

impl Indexes {
    /// Builds every library's indexes in the background, one library at a time, its names first,
    /// so that no search after a restart waits for one.
    pub fn build_all(self: &Arc<Self>, db: &Arc<Database>) {
        let (indexes, db) = (self.clone(), db.clone());
        tokio::spawn(async move {
            let libraries = match db.read(|conn| versions(conn, Index::VERSION)).await {
                Ok(versions) => versions.into_keys(),
                Err(error) => {
                    tracing::warn!(%error, "cannot list the libraries to build search indexes for");
                    return;
                }
            };
            for library in libraries {
                if let Err(error) = indexes.index(&db, library).await {
                    tracing::warn!(%error, library, "cannot build a search index");
                }
                if let Err(error) = indexes.lyrics(&db, library).await {
                    tracing::warn!(%error, library, "cannot build a lyric index");
                }
            }
        });
    }

    /// The names index of `library`, as [`Libraries::get`] gives it.
    pub async fn index(&self, db: &Arc<Database>, library: i64) -> Result<Arc<Index>, DbError> {
        self.names.get(db, library).await
    }

    /// The lyric index of `library`, as [`Libraries::get`] gives it.
    pub async fn lyrics(
        &self,
        db: &Arc<Database>,
        library: i64,
    ) -> Result<Arc<LyricIndex>, DbError> {
        self.lyrics.get(db, library).await
    }
}

/// An index each library keeps in memory, read from the database and built again when the
/// library's version of what it indexes moves.
trait Searched: Default + Send + Sync + Sized + 'static {
    /// The column of `libraries` that the triggers bump whenever what this indexes changes.
    const VERSION: &'static str;

    /// Reads `library` and indexes it, with the library's version that the index reflects: the
    /// version is read first, so whatever the index holds is at least that recent, and a change
    /// it already holds can only make it rebuild once more.
    fn build(
        db: Arc<Database>,
        library: i64,
    ) -> impl Future<Output = Result<(i64, Self), DbError>> + Send;
}

/// Every library's index of one kind.
struct Libraries<T> {
    libraries: Mutex<HashMap<i64, Arc<Slot<T>>>>,
}

impl<T> Default for Libraries<T> {
    fn default() -> Self {
        Libraries { libraries: Mutex::default() }
    }
}

/// One library's index, once built, and the lock that keeps its builds one at a time.
struct Slot<T> {
    built: RwLock<Option<Built<T>>>,
    /// Held while building, so a library builds one index of a kind at a time.
    building: Arc<tokio::sync::Mutex<()>>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Slot { built: RwLock::default(), building: Arc::default() }
    }
}

/// A built index, and what decides when to build it again.
struct Built<T> {
    index: Arc<T>,
    /// The library's version when the index was read.
    version: i64,
    /// When the index was ready, and how long it took to build.
    ready: Instant,
    took: Duration,
}

impl<T> Clone for Built<T> {
    fn clone(&self) -> Self {
        Built {
            index: self.index.clone(),
            version: self.version,
            ready: self.ready,
            took: self.took,
        }
    }
}

impl<T> Built<T> {
    /// Whether to rebuild for a library whose version is `version`.
    fn due(&self, version: i64) -> bool {
        self.version != version && self.ready.elapsed() >= self.took * REBUILD_SPACING
    }
}

impl<T: Searched> Libraries<T> {
    /// The index of `library`, starting a rebuild if what it indexes has changed since it was
    /// built. Waits only while the library's first index is built. A library deleted since the
    /// request was let in has an empty index, and the indexes of deleted libraries are dropped.
    async fn get(&self, db: &Arc<Database>, library: i64) -> Result<Arc<T>, DbError> {
        let versions = db.read(|conn| versions(conn, T::VERSION)).await?;
        let (slot, version) = {
            let mut libraries = self.libraries.lock().recover();
            libraries.retain(|id, _| versions.contains_key(id));
            let Some(&version) = versions.get(&library) else {
                return Ok(Arc::default());
            };
            (libraries.entry(library).or_default().clone(), version)
        };
        let built = slot.built.read().recover().clone();
        if let Some(built) = built {
            if built.due(version)
                && let Ok(guard) = slot.building.clone().try_lock_owned()
            {
                let (db, slot) = (db.clone(), slot.clone());
                tokio::spawn(async move {
                    match build::<T>(db, library).await {
                        Ok(built) => *slot.built.write().recover() = Some(built),
                        Err(error) => {
                            tracing::warn!(%error, library, kind = T::VERSION, "cannot rebuild a search index")
                        }
                    }
                    drop(guard);
                });
            }
            return Ok(built.index);
        }
        // The first search waits for the index, and so does any that arrives meanwhile.
        let _guard = slot.building.lock().await;
        if let Some(built) = slot.built.read().recover().clone() {
            return Ok(built.index);
        }
        let built = build::<T>(db.clone(), library).await?;
        let index = built.index.clone();
        *slot.built.write().recover() = Some(built);
        Ok(index)
    }
}

/// Every library's version in `column`, by library.
fn versions(conn: &Connection, column: &'static str) -> rusqlite::Result<HashMap<i64, i64>> {
    conn.prepare_cached(&format!("SELECT id, {column} FROM libraries"))?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

/// Builds `library`'s index, timed.
async fn build<T: Searched>(db: Arc<Database>, library: i64) -> Result<Built<T>, DbError> {
    let started = Instant::now();
    let (version, index) = T::build(db, library).await?;
    Ok(Built { index: Arc::new(index), version, ready: Instant::now(), took: started.elapsed() })
}

/// Runs `work`, CPU-bound, off the async workers. Work that panics is [`DbError::Aborted`].
async fn off_the_workers<R: Send + 'static>(
    library: i64,
    work: impl FnOnce() -> R + Send + 'static,
) -> Result<R, DbError> {
    tokio::task::spawn_blocking(work).await.map_err(|error| {
        tracing::error!(%error, library, "building a search index did not complete");
        DbError::Aborted
    })
}

impl Searched for Index {
    const VERSION: &'static str = "names_version";

    async fn build(db: Arc<Database>, library: i64) -> Result<(i64, Index), DbError> {
        let (version, builder) = db
            .read(move |conn| {
                // One snapshot, so the version is exactly the one the documents reflect. Each
                // name is folded into the builder as it is read, so no list of all of them is held.
                let version = library_number(conn, library, Self::VERSION)?;
                let mut builder = IndexBuilder::default();
                documents(conn, library, |document| builder.add(document))?;
                Ok((version, builder))
            })
            .await?;
        // Sorting and laying out the index takes hundreds of milliseconds of CPU at full scale,
        // more on a Pi, so off the async workers.
        let index = off_the_workers(library, move || builder.finish()).await?;
        Ok((version, index))
    }
}

impl Searched for LyricIndex {
    const VERSION: &'static str = "lyrics_version";

    /// A batch of tracks' lyrics at a time, each its own read, folded into the index off the
    /// async workers between reads: lyrics outweigh names many times over, so they are never all
    /// held at once, and no reader is held for the whole build.
    async fn build(db: Arc<Database>, library: i64) -> Result<(i64, LyricIndex), DbError> {
        let version = db.read(move |conn| library_number(conn, library, Self::VERSION)).await?;
        let mut builder = LyricIndexBuilder::default();
        let mut after = i64::MIN;
        loop {
            let batch = db.read(move |conn| lyrics_after(conn, library, after)).await?;
            let Some(&(last, _)) = batch.last() else { break };
            let full = batch.len() == LYRICS_BATCH;
            after = last;
            builder = off_the_workers(library, move || {
                for (track, lines) in batch {
                    builder.add(track, lines);
                }
                builder
            })
            .await?;
            if !full {
                break;
            }
        }
        let index = off_the_workers(library, move || builder.finish()).await?;
        Ok((version, index))
    }
}

/// The lyrics of the next [`LYRICS_BATCH`] tracks of `library` with any, by ID, after track
/// `after`, each as its lines.
fn lyrics_after(
    conn: &Connection,
    library: i64,
    after: i64,
) -> rusqlite::Result<Vec<(i64, Vec<String>)>> {
    conn.prepare_cached(
        "SELECT track_id, plain, synced FROM track_lyrics \
         WHERE library_id = ?1 AND track_id > ?2 ORDER BY track_id LIMIT ?3",
    )?
    .query_map(rusqlite::params![library, after, LYRICS_BATCH], |row| {
        let plain: Option<String> = row.get(1)?;
        let synced: Option<db::Json<Vec<SyncedLine>>> = row.get(2)?;
        Ok((row.get(0)?, lyric_lines(plain, synced.map(|db::Json(lines)| lines))))
    })?
    .collect()
}

/// A track's lyrics as lines, as the lyric index and what shows a match number them: each timed
/// line, or plain lyrics split at their line breaks.
pub(super) fn lyric_lines(plain: Option<String>, synced: Option<Vec<SyncedLine>>) -> Vec<String> {
    match (synced, plain) {
        (Some(lines), _) => lines.into_iter().map(|line| line.text).collect(),
        (None, Some(plain)) => jewelcase_core::plain_lines(&plain).map(str::to_owned).collect(),
        (None, None) => Vec::new(),
    }
}

#[cfg(test)]
mod tests;
