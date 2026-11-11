//! Each library's search index, in memory and tagged with the library's names version, which the
//! catalog's triggers bump whenever a track, album or artist is added, deleted or renamed, and
//! never for analysis, art or any other write (`0007_catalog_counts.sql`). Every library's index
//! is built in the background as the server starts. A search whose library's names have changed
//! since starts a rebuild in the background and answers from the index it has, so a search waits
//! only while its library's first index is built. A deleted library's index is dropped by the
//! next search. The index is rebuildable data (`design/database.md` §7).
//!
//! Rebuilds are spaced by how long the last one took, so a library whose names change
//! constantly, as they do during a scan, costs a tenth of a core at most, on any host and at any
//! size. A small library is rebuilt within moments of a change; a large one on slow hardware,
//! within seconds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use jewelcase_catalog::search::documents;
use jewelcase_core::{Index, IndexBuilder};
use rusqlite::Connection;

use super::tagged::TaggedSets;
use crate::api::page::library_number;
use crate::db::{Database, DbError};
use jewelcase_core::Recover;

/// A rebuild waits this many times as long as the last one took.
const REBUILD_SPACING: u32 = 10;

/// Every library's search index, and the rows recent searches' tags were read as.
#[derive(Default)]
pub struct Indexes {
    libraries: Mutex<HashMap<i64, Arc<Slot>>>,
    pub tagged: TaggedSets,
}

/// One library's index, once built, and the lock that keeps its builds one at a time.
#[derive(Default)]
struct Slot {
    built: RwLock<Option<Built>>,
    /// Held while building, so a library builds one index at a time.
    building: Arc<tokio::sync::Mutex<()>>,
}

/// A built index, and what decides when to build it again.
#[derive(Clone)]
struct Built {
    index: Arc<Index>,
    /// The library's names version when the index was read.
    version: i64,
    /// When the index was ready, and how long it took to build.
    ready: Instant,
    took: Duration,
}

impl Built {
    /// Whether to rebuild for a library whose names version is `version`.
    fn due(&self, version: i64) -> bool {
        self.version != version && self.ready.elapsed() >= self.took * REBUILD_SPACING
    }
}

impl Indexes {
    /// Builds every library's index in the background, one library at a time, so that no
    /// search after a restart waits for one.
    pub fn build_all(self: &Arc<Self>, db: &Arc<Database>) {
        let (indexes, db) = (self.clone(), db.clone());
        tokio::spawn(async move {
            let libraries = match db.read(versions).await {
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
            }
        });
    }

    /// The index of `library`, starting a rebuild if its names have changed since it was built.
    /// Waits only while the library's first index is built. A library deleted since the request
    /// was let in has an empty index, and the indexes of deleted libraries are dropped.
    pub async fn index(&self, db: &Arc<Database>, library: i64) -> Result<Arc<Index>, DbError> {
        let versions = db.read(versions).await?;
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
                    match build(&db, library).await {
                        Ok(built) => *slot.built.write().recover() = Some(built),
                        Err(error) => {
                            tracing::warn!(%error, library, "cannot rebuild a search index")
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
        let built = build(db, library).await?;
        let index = built.index.clone();
        *slot.built.write().recover() = Some(built);
        Ok(index)
    }
}

/// Every library's names version, by library.
fn versions(conn: &Connection) -> rusqlite::Result<HashMap<i64, i64>> {
    conn.prepare_cached("SELECT id, names_version FROM libraries")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

/// Reads the library's names and indexes them. A build that panics is [`DbError::Aborted`].
async fn build(db: &Database, library: i64) -> Result<Built, DbError> {
    let started = Instant::now();
    let (version, builder) = db
        .read(move |conn| {
            // One snapshot, so the version is exactly the one the documents reflect. Each name is
            // folded into the builder as it is read, so no list of all of them is held.
            let version = library_number(conn, library, "names_version")?;
            let mut builder = IndexBuilder::default();
            documents(conn, library, |document| builder.add(document))?;
            Ok((version, builder))
        })
        .await?;
    // Sorting and laying out the index takes hundreds of milliseconds of CPU at full scale, more
    // on a Pi, so off the async workers.
    let index = tokio::task::spawn_blocking(move || builder.finish()).await.map_err(|error| {
        tracing::error!(%error, library, "building a search index did not complete");
        DbError::Aborted
    })?;
    Ok(Built { index: Arc::new(index), version, ready: Instant::now(), took: started.elapsed() })
}

#[cfg(test)]
mod tests;
