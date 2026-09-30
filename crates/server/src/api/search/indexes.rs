//! Each library's search index, in memory and tagged with the change-feed position it
//! reflects. A search whose library has moved on since starts a rebuild in the background and
//! answers from the index it has, so no search waits on one; only a library's very first search
//! waits for its index to be built. The index is rebuildable data (`design/database.md` §7).
//!
//! Rebuilds are spaced by how long the last one took, so a library that changes constantly, as
//! it does during a scan, costs a tenth of a core at most, on any host and at any size. A small
//! library is rebuilt within moments of a change; a large one on slow hardware, within seconds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use jewelcase_core::search::{Document, Index, Kind};
use rusqlite::Connection;

use crate::db::{Database, DbError};

/// A rebuild waits this many times as long as the last one took.
const REBUILD_SPACING: u32 = 10;

/// Every library's search index.
#[derive(Default)]
pub struct Indexes {
    libraries: Mutex<HashMap<i64, Arc<Slot>>>,
}

#[derive(Default)]
struct Slot {
    built: RwLock<Option<Built>>,
    /// Held while building, so a library builds one index at a time.
    building: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Clone)]
struct Built {
    index: Arc<Index>,
    /// The library's change-feed position when the index was read.
    position: i64,
    /// When the index was ready, and how long it took to build.
    ready: Instant,
    took: Duration,
}

impl Built {
    /// Whether to rebuild for a library now at `position`.
    fn due(&self, position: i64) -> bool {
        self.position != position && self.ready.elapsed() >= self.took * REBUILD_SPACING
    }
}

impl Indexes {
    /// The library's index, starting a rebuild if the library has changed since `position`.
    pub async fn index(
        &self,
        db: &Arc<Database>,
        library: i64,
        position: i64,
    ) -> Result<Arc<Index>, DbError> {
        let slot = self.libraries.lock().unwrap().entry(library).or_default().clone();
        let built = slot.built.read().unwrap().clone();
        if let Some(built) = built {
            if built.due(position)
                && let Ok(guard) = slot.building.clone().try_lock_owned()
            {
                let (db, slot) = (db.clone(), slot.clone());
                tokio::spawn(async move {
                    match build(&db, library).await {
                        Ok(built) => *slot.built.write().unwrap() = Some(built),
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
        if let Some(built) = slot.built.read().unwrap().clone() {
            return Ok(built.index);
        }
        let built = build(db, library).await?;
        let index = built.index.clone();
        *slot.built.write().unwrap() = Some(built);
        Ok(index)
    }
}

/// Reads the library's names and indexes them.
async fn build(db: &Database, library: i64) -> Result<Built, DbError> {
    let started = Instant::now();
    let (position, documents) = db
        .read(move |conn| {
            // The position first: a change landing between the two reads makes the index look
            // older than it is, so the next search rebuilds it again, never the reverse.
            let position = feed_position(conn, library)?;
            let documents = conn
                .prepare_cached(
                    "SELECT 0, id, title FROM tracks WHERE library_id = ?1 \
                     UNION ALL SELECT 1, id, title FROM albums WHERE library_id = ?1 AND title IS NOT NULL \
                     UNION ALL SELECT 2, id, name FROM artists WHERE library_id = ?1 AND name IS NOT NULL",
                )?
                .query_map([library], |row| {
                    Ok(Document {
                        kind: Kind::ALL[usize::from(row.get::<_, u8>(0)?)],
                        id: row.get(1)?,
                        name: row.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((position, documents))
        })
        .await?;
    // A second of CPU at full scale, so off the async workers.
    let index = tokio::task::spawn_blocking(move || Index::build(documents))
        .await
        .expect("building a search index does not panic");
    Ok(Built { index: Arc::new(index), position, ready: Instant::now(), took: started.elapsed() })
}

pub fn feed_position(conn: &Connection, library: i64) -> rusqlite::Result<i64> {
    conn.prepare_cached("SELECT ifnull(max(seq), 0) FROM library_changes WHERE library_id = ?1")?
        .query_row([library], |row| row.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(position: i64, took: Duration) -> Built {
        Built { index: Arc::new(Index::build([])), position, ready: Instant::now(), took }
    }

    #[test]
    fn rebuilds_are_spaced_by_how_long_the_last_took() {
        // A slow build just finished: the library has changed, but it is too soon.
        assert!(!built(1, Duration::from_secs(60)).due(2));
        // A quick one is rebuilt at once.
        assert!(built(1, Duration::ZERO).due(2));
        // Nothing has changed.
        assert!(!built(1, Duration::ZERO).due(1));
    }
}
