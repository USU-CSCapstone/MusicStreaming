//! What the containers a search is narrowed to hold (`within`, `requirements/search.md` §5), read
//! by the catalog (`jewelcase_catalog::within`) and kept as a tag filter's rows are, so a search
//! typed a letter at a time inside a large genre reads its rows once.

use std::sync::Arc;

use jewelcase_catalog::within::{Within, read};
use rusqlite::Connection;

use super::tagged::{Kept, Sets};
use crate::db::{Database, DbError};

/// What containers hold, by library and container.
#[derive(Default)]
pub struct Containers {
    kept: Kept<(i64, Within), Option<Sets>>,
}

impl Containers {
    /// What `within` holds in `library` as `user` reaches it ([`read`]), kept while the library's
    /// change feed stays put. A playlist is the user's own, and its changes are not in the feed,
    /// so it is read each time: one range of its items, which costs little however long it is.
    pub async fn get(
        &self,
        db: &Database,
        user: i64,
        library: i64,
        within: Within,
    ) -> Result<Option<Sets>, DbError> {
        let kept = self.kept.clone();
        db.read(move |conn| {
            let read = |conn: &Connection| Ok(read(conn, user, library, within)?.map(Arc::new));
            match within {
                Within::Playlist(_) => read(conn),
                _ => kept.get(conn, library, (library, within), read),
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use jewelcase_core::Kind;

    use super::*;
    use crate::api::testing::catalog;
    use crate::db::feed::{self, Entity, Op};
    use crate::db::testing::Track;

    #[tokio::test]
    async fn what_a_container_holds_is_kept_until_the_library_changes() {
        let (_temp, db, _app) = catalog::app().await;
        let containers = Containers::default();
        let get = |within| {
            let (containers, db) = (&containers, &db);
            async move { containers.get(db, 1, 1, within).await.unwrap() }
        };
        let album = get(Within::Album(102)).await.unwrap();
        assert_eq!(album[Kind::Track as usize], HashSet::from([1005]));
        assert!(Arc::ptr_eq(&album, &get(Within::Album(102)).await.unwrap()), "kept");
        assert_eq!(get(Within::Album(999)).await, None, "one the library lacks");

        // Kid A gains a track, which the feed records.
        db.write(|tx| {
            Track::new(1007, 1, 102).insert(tx)?;
            feed::record(tx, 1, Entity::Track, 1007, Op::Upsert, 0)
        })
        .await
        .unwrap();
        let changed = get(Within::Album(102)).await.unwrap();
        assert_eq!(changed[Kind::Track as usize], HashSet::from([1005, 1007]));
    }

    #[tokio::test]
    async fn a_playlist_is_read_each_time() {
        let (_temp, db, _app) = catalog::app().await;
        db.write(|tx| {
            tx.execute_batch(
                "INSERT INTO playlists
                     (id, library_id, user_id, title, folded, sort_key, created_at, modified_at)
                     VALUES (61, 1, 1, 'Night', 'night', CAST('night' AS BLOB), 0, 0);
                 INSERT INTO playlist_items (playlist_id, key, library_id, track_id, added_at)
                     VALUES (61, 1, 1, 1001, 0);",
            )
        })
        .await
        .unwrap();
        let containers = Containers::default();
        let first = containers.get(&db, 1, 1, Within::Playlist(61)).await.unwrap().unwrap();
        assert_eq!(first[Kind::Track as usize], HashSet::from([1001]));
        let again = containers.get(&db, 1, 1, Within::Playlist(61)).await.unwrap().unwrap();
        assert!(!Arc::ptr_eq(&first, &again), "its changes are not in the feed");
        assert_eq!(
            containers.get(&db, 2, 1, Within::Playlist(61)).await.unwrap(),
            None,
            "another's"
        );
    }
}
