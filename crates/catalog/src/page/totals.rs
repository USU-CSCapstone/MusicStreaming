//! Counted totals remembered between pages: a device counts a tag-filtered list's rows for every
//! page of it, and the rows do not change until a write reaches the file.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rusqlite::Connection;

use super::Source;
use super::keyset::{count, count_query};

/// How many totals are kept: a list's pages ask for one total, and a few open lists, each with
/// the filters a person changes between, are far fewer than this.
const KEPT: usize = 64;

/// The totals of the lists a device has counted, the least recently asked for let go first. Only
/// the holder of a database file that nothing else writes may keep one, and it must [`clear`]
/// the totals before any write is seen, so a total is never older than the rows it counts.
///
/// [`clear`]: Totals::clear
#[derive(Default)]
pub struct Totals(Mutex<VecDeque<(String, i64)>>);

impl Totals {
    /// Lets go of every total, as a write to the file makes them untrue.
    pub fn clear(&self) {
        self.kept().clear();
    }

    /// The kept totals, oldest first. A request's reads happen one at a time here, so the lock is
    /// uncontended; it only lets a request that holds one move between threads.
    fn kept(&self) -> MutexGuard<'_, VecDeque<(String, i64)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The totals as `user` asks for them: a user's hidden tracks change what a list counts, and
    /// they are in the query's parameters, but each user has totals of their own besides.
    pub fn of(&self, user: i64) -> Memo<'_> {
        Memo { totals: self, user }
    }
}

/// One user's view of a [`Totals`].
#[derive(Clone, Copy)]
pub struct Memo<'a> {
    totals: &'a Totals,
    user: i64,
}

impl Memo<'_> {
    /// How many rows `source` holds: the remembered total of the same query with the same
    /// parameters for this user, or else a count, which is remembered.
    pub(super) fn count(&self, conn: &Connection, source: &Source) -> rusqlite::Result<i64> {
        let (sql, params) = count_query(source);
        let key = format!("{}\0{sql}\0{params:?}", self.user);
        let mut kept = self.totals.kept();
        if let Some(at) = kept.iter().position(|(known, _)| *known == key) {
            let entry = kept.remove(at).expect("the position was found");
            let total = entry.1;
            kept.push_back(entry);
            return Ok(total);
        }
        drop(kept);
        let total = count(conn, source)?;
        let mut kept = self.totals.kept();
        if kept.len() == KEPT {
            kept.pop_front();
        }
        kept.push_back((key, total));
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::types::Value;

    use super::*;
    use crate::page::{Request, Sort, Total, Unknown};

    /// A sort by `n` alone, which never meets an unknown value.
    const BY_N: Sort = Sort { label: "n", columns: &["n"], unknown: Unknown::Never };

    /// A source of the rows of `rows` whose `n` is above `above`, counted.
    fn above(above: i64) -> Source {
        Source {
            select: &["n"],
            from: "rows".to_owned(),
            joins: "",
            filter: "n > ?".to_owned(),
            params: vec![Value::from(above)],
            total: Total::Counted,
            counting: None,
            back: None,
            walk: None,
        }
    }

    /// A database whose `rows` has ten rows, 1 to 10.
    fn ten() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE rows (n INTEGER); \
             WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10) \
             INSERT INTO rows SELECT i FROM n",
        )
        .unwrap();
        conn
    }

    /// The total of the page of `source` that user `user` reads, remembered in `totals`.
    fn total(conn: &Connection, totals: &Totals, user: i64, source: &Source) -> i64 {
        let request = Request::new(&BY_N, crate::page::Order::Asc, None, Some(1))
            .unwrap()
            .memoized(Some(totals.of(user)));
        request.read(conn, source, |row| row.get::<_, i64>(0)).unwrap().total
    }

    #[test]
    fn a_total_asked_again_reads_no_rows_until_something_is_written() {
        let (conn, totals) = (ten(), Totals::default());
        assert_eq!(total(&conn, &totals, 1, &above(0)), 10);
        // A row written behind the memo's back is not counted: the second ask read no rows.
        conn.execute("DELETE FROM rows WHERE n > 5", []).unwrap();
        assert_eq!(total(&conn, &totals, 1, &above(0)), 10);
        // Another question is counted.
        assert_eq!(total(&conn, &totals, 1, &above(2)), 3);
        totals.clear();
        assert_eq!(total(&conn, &totals, 1, &above(0)), 5);
    }

    #[test]
    fn each_user_keeps_totals_of_their_own() {
        let (conn, totals) = (ten(), Totals::default());
        assert_eq!(total(&conn, &totals, 1, &above(0)), 10);
        conn.execute("DELETE FROM rows WHERE n > 5", []).unwrap();
        assert_eq!(total(&conn, &totals, 1, &above(0)), 10);
        assert_eq!(total(&conn, &totals, 2, &above(0)), 5);
    }

    #[test]
    fn the_least_recently_asked_total_is_let_go_first() {
        let (conn, totals) = (ten(), Totals::default());
        for above in 0..i64::try_from(KEPT).unwrap() {
            total(&conn, &totals, 1, &self::above(above - 100));
        }
        // Asking again makes the oldest the newest, so the next new one pushes out the second.
        assert_eq!(total(&conn, &totals, 1, &above(-100)), 10);
        total(&conn, &totals, 1, &above(-1000));
        conn.execute("DELETE FROM rows WHERE n > 5", []).unwrap();
        assert_eq!(total(&conn, &totals, 1, &above(-100)), 10, "kept");
        assert_eq!(total(&conn, &totals, 1, &above(-99)), 5, "let go");
    }

    #[test]
    fn a_list_answered_with_totals_remembers_its_counted_total_until_they_are_cleared() {
        use crate::page::plans::device;
        use crate::query::answer;
        use serde_json::json;

        let conn = device();
        conn.execute_batch(
            "INSERT INTO hidden_tracks SELECT 1, 1, id, id FROM tracks WHERE id <= 20",
        )
        .unwrap();
        let totals = Totals::default();
        // Each list's counted total, and a statement that lowers it.
        let lists = [
            ("listTracks", json!({ "tagAny": "2,3" }), "DELETE FROM track_tags WHERE tag_id = 2"),
            ("listAlbums", json!({ "tagAny": "2,3" }), "DELETE FROM album_tags WHERE tag_id = 2"),
            ("listArtists", json!({ "tagAny": "2,3" }), "DELETE FROM artist_tags WHERE tag_id = 2"),
            (
                "listTags",
                json!({ "type": "genre" }),
                "DELETE FROM tags WHERE id BETWEEN 301 AND 320",
            ),
            ("listPlaylists", json!({}), "DELETE FROM playlists WHERE id <= 10"),
            ("listHiddenTracks", json!({}), "DELETE FROM hidden_tracks WHERE track_id <= 10"),
        ];
        for (name, mut params, lower) in lists {
            params["limit"] = json!(1);
            let total = |user| {
                let page = answer(&conn, user, 1, name, &params.to_string(), None, Some(&totals));
                let page: serde_json::Value = serde_json::from_str(&page.unwrap()).unwrap();
                page["total"].as_i64().unwrap()
            };
            let before = total(1);
            assert!(before > 1, "{name}");
            // Rows deleted behind the memo's back are not missed: nothing was read.
            conn.execute(lower, []).unwrap();
            assert_eq!(total(1), before, "{name}");
            assert_ne!(total(2), before, "{name}: another user's total is counted afresh");
            totals.clear();
            assert!(total(1) < before, "{name}");
        }
    }
}
