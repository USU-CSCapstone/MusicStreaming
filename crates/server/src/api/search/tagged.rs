//! The rows that carry a search's tags, kept while the library's catalog stays as it is, so a
//! search typed a letter at a time reads a large tag's rows once rather than on every letter
//! (`design/database.md` §4).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use jewelcase_core::{Kind, Recover};
use rusqlite::Connection;

use crate::api::tag_filter::{TagFilter, Tagged, Tags};
use crate::api::{AnyId, albums, artists, tracks};
use crate::db::{Database, DbError};

/// How many filters' rows are kept, the least recently used going first. One genre of a large
/// library is a few megabytes, and a search typed a letter at a time keeps its filter.
const KEPT: usize = 4;

/// For each kind in `Kind::ALL` order, the IDs of the rows of it that carry a search's tags: none
/// for a kind it does not search.
pub type Sets = Arc<[HashSet<i64>; 3]>;

/// What kept rows are found by: the library, the tags as the filter's `tagAll` and `tagAny` name
/// them, each sorted and each tag once, and the kinds searched, sorted.
#[derive(Clone, PartialEq, Eq)]
struct Key {
    library: i64,
    all: Vec<Option<i64>>,
    any: Vec<Option<i64>>,
    kinds: Vec<Kind>,
}

/// Rows read for searches, each with its key and the head of its library's change feed when it
/// was read, the most recently used last.
#[derive(Default)]
pub struct TaggedSets {
    kept: Arc<Mutex<Vec<(Key, i64, Sets)>>>,
}

impl TaggedSets {
    /// The rows of `library` of each of `kinds` that carry the tags `filter` names, or `None`
    /// where it names none. Kept rows are used while the library's change feed has nothing newer
    /// than when they were read, since every change to what a row carries writes to it and its
    /// positions are never reused; otherwise they are read again, in one snapshot with the head.
    pub async fn get(
        &self,
        db: &Database,
        library: i64,
        filter: &TagFilter,
        kinds: &[Kind],
    ) -> Result<Option<Sets>, DbError> {
        if filter.is_empty() {
            return Ok(None);
        }
        let ids = |ids: &[AnyId]| {
            let mut ids: Vec<Option<i64>> = ids.iter().map(|id| id.0).collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };
        let mut kinds = kinds.to_vec();
        kinds.sort_unstable();
        kinds.dedup();
        let key = Key { library, all: ids(&filter.all), any: ids(&filter.any), kinds };
        let (kept, filter) = (self.kept.clone(), filter.clone());
        let sets = db
            .read(move |conn| {
                let head = head(conn, library)?;
                if let Some(sets) = used(&mut kept.lock().recover(), &key, head) {
                    return Ok(sets);
                }
                let sets: Sets = Arc::new(read(conn, library, &filter, &key.kinds)?);
                let mut kept = kept.lock().recover();
                kept.retain(|(kept, ..)| *kept != key);
                if kept.len() == KEPT {
                    kept.remove(0);
                }
                kept.push((key, head, sets.clone()));
                Ok(sets)
            })
            .await?;
        Ok(Some(sets))
    }
}

/// The rows kept for `key` at feed head `head`, moved to most recently used.
fn used(kept: &mut Vec<(Key, i64, Sets)>, key: &Key, head: i64) -> Option<Sets> {
    let at = kept.iter().position(|(kept, at, _)| kept == key && *at == head)?;
    let entry = kept.remove(at);
    let sets = entry.2.clone();
    kept.push(entry);
    Some(sets)
}

/// The position of `library`'s latest change, 0 when it has none.
fn head(conn: &Connection, library: i64) -> rusqlite::Result<i64> {
    conn.prepare_cached("SELECT ifnull(max(seq), 0) FROM library_changes WHERE library_id = ?1")?
        .query_row([library], |row| row.get(0))
}

/// Reads the rows of `library` of each of `kinds` that carry the tags `filter` names, as each
/// list's own filter links them ([`Tags::ids`]).
fn read(
    conn: &Connection,
    library: i64,
    filter: &TagFilter,
    kinds: &[Kind],
) -> rusqlite::Result<[HashSet<i64>; 3]> {
    let mut sets: [HashSet<i64>; 3] = Default::default();
    for &kind in kinds {
        let tagged: &Tagged = match kind {
            Kind::Track => &tracks::TAGGED,
            Kind::Album => &albums::TAGGED,
            Kind::Artist => &artists::TAGGED,
        };
        let ids = Tags::read(conn, library, tagged, filter)?.ids(conn, tagged)?;
        sets[kind as usize] = ids.unwrap_or_default();
    }
    Ok(sets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::testing::catalog;
    use crate::db::feed::{self, Entity, Op};

    fn ids(ids: &[i64]) -> Vec<AnyId> {
        ids.iter().map(|&id| AnyId(Some(id))).collect()
    }

    fn filter(any: &[i64]) -> TagFilter {
        TagFilter { any: ids(any), ..TagFilter::default() }
    }

    #[tokio::test]
    async fn rows_are_kept_until_the_library_changes() {
        let (_temp, db, _app) = catalog::app().await;
        let sets = TaggedSets::default();
        let tracks = [Kind::Track];
        let first = sets.get(&db, 1, &filter(&[30, 32]), &tracks).await.unwrap().unwrap();
        assert_eq!(first[0], HashSet::from([1001]));
        let again = sets.get(&db, 1, &filter(&[32, 30, 30]), &tracks).await.unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &again), "the same tags in another order are kept");
        let all = sets.get(&db, 1, &filter(&[30, 32]), &Kind::ALL).await.unwrap().unwrap();
        assert!(!Arc::ptr_eq(&first, &all), "more kinds are read again");
        assert_eq!(all[1], HashSet::from([101]));
        let rock = sets.get(&db, 1, &filter(&[30]), &tracks).await.unwrap().unwrap();

        // Rock or Pop is track 1001, and Rock and Pop no track, so the same tags as `tagAll` are
        // another filter.
        let any = sets.get(&db, 1, &filter(&[30, 31]), &tracks).await.unwrap().unwrap();
        let both = TagFilter { all: ids(&[30, 31]), ..TagFilter::default() };
        let all = sets.get(&db, 1, &both, &tracks).await.unwrap().unwrap();
        assert!(!Arc::ptr_eq(&any, &all), "tags all named are kept apart from any named");
        assert_eq!((&any[0], &all[0]), (&HashSet::from([1001]), &HashSet::new()));

        // Track 1002 becomes Rock, which the feed records.
        db.write(|tx| {
            tx.execute(
                "INSERT INTO track_tags (library_id, track_id, tag_id, tag_name) \
                 VALUES (1, 1002, 30, 'Rock')",
                [],
            )?;
            feed::record(tx, 1, Entity::Track, 1002, Op::Upsert, 0)
        })
        .await
        .unwrap();
        let changed = sets.get(&db, 1, &filter(&[30]), &tracks).await.unwrap().unwrap();
        assert_eq!((rock[0].len(), &changed[0]), (1, &HashSet::from([1001, 1002])));
        assert!(sets.get(&db, 1, &TagFilter::default(), &tracks).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn the_least_recently_used_goes_first() {
        let (_temp, db, _app) = catalog::app().await;
        let sets = TaggedSets::default();
        let get = |ids: &'static [i64]| {
            let (sets, db) = (&sets, &db);
            async move { sets.get(db, 1, &filter(ids), &[Kind::Track]).await.unwrap().unwrap() }
        };
        let rock = get(&[30]).await;
        for ids in [&[31][..], &[32], &[30, 31]] {
            get(ids).await;
        }
        assert!(Arc::ptr_eq(&rock, &get(&[30]).await), "four are kept");
        get(&[30, 32]).await;
        let calm = get(&[32]).await;
        assert!(Arc::ptr_eq(&rock, &get(&[30]).await), "used since, so kept");
        // Three more push out the three used least recently, Calm last of them.
        for ids in [&[31, 32][..], &[30, 31, 32], &[31]] {
            get(ids).await;
        }
        assert!(!Arc::ptr_eq(&calm, &get(&[32]).await), "the least recently used went");
    }
}
