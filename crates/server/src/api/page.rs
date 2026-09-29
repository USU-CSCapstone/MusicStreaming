//! Keyset paging for browse lists (`design/database.md` §4).
//!
//! A page continues from the last row's sort values, never from an offset, so every page costs
//! what the first does. Rows with no sortable value, such as an album with no release date,
//! sort after all the others in both orders (`requirements/conventions.md` §4). Such a list is
//! read in two parts, the known values and then the unknown ones, and each part is one range
//! of the sort's index. Only a sort's first column is treated so; in the columns that break its
//! ties, an unknown value sorts like any other.

use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Row, params_from_iter};
use serde::{Deserialize, Serialize};

use super::{Code, Problem, cursor};

pub const DEFAULT_LIMIT: u32 = 100;
pub const MAX_LIMIT: u32 = 1000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Order {
    #[default]
    Asc,
    Desc,
}

/// How the first column of a sort marks a row with no sortable value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unknown {
    /// Every row has one.
    Never,
    /// `NULL`, such as a missing release date.
    Null,
    /// An empty sort key: the unknown artist or album.
    Empty,
}

pub struct Sort {
    /// Names the list and sort in its cursors, such as `albums.name`.
    pub label: &'static str,
    /// The columns to order by. The last is unique, so the order is total, and an index should
    /// cover them after `library_id`.
    pub columns: &'static [&'static str],
    pub unknown: Unknown,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub total: i64,
}

/// What a list reads: `SELECT {select} FROM {from} WHERE {filter}`, with `params` for the `?`s
/// in `from` and `filter`, in order.
pub struct Source {
    /// Columns for the row mapper, which reads them from the front of each row.
    pub select: &'static [&'static str],
    pub from: String,
    pub filter: String,
    pub params: Vec<Value>,
}

/// Checks `limit`, which the spec bounds from 1 to 1000.
pub fn limit(limit: Option<u32>) -> Result<usize, Problem> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if (1..=MAX_LIMIT).contains(&limit) {
        Ok(limit as usize)
    } else {
        Err(Problem::new(Code::ValidationFailed).detail("limit must be from 1 to 1000"))
    }
}

/// SQL for a millisecond timestamp column as an RFC 3339 string, with the milliseconds computed
/// in integer arithmetic so they come out exact.
macro_rules! timestamp {
    ($column:literal) => {
        concat!(
            "strftime('%Y-%m-%dT%H:%M:%S', ",
            $column,
            " / 1000, 'unixepoch') || printf('.%03dZ', ",
            $column,
            " % 1000)"
        )
    };
}
pub(crate) use timestamp;

/// The library's stored count in `column`, such as `album_count`, which the scanner keeps
/// current; `None` if there is no such library.
pub fn library_count(
    conn: &Connection,
    library: i64,
    column: &'static str,
) -> rusqlite::Result<Option<i64>> {
    conn.prepare_cached(&format!("SELECT {column} FROM libraries WHERE id = ?1"))?
        .query_row([library], |row| row.get(0))
        .optional()
}

impl Sort {
    fn label(&self, order: Order) -> String {
        let order = match order {
            Order::Asc => "asc",
            Order::Desc => "desc",
        };
        format!("{}.{order}", self.label)
    }

    /// Reads a request's cursor, before the query runs.
    pub fn after(&self, cursor: Option<&str>, order: Order) -> Result<Option<Vec<Value>>, Problem> {
        cursor
            .map(|text| cursor::decode(text, &self.label(order), self.columns.len()))
            .transpose()
    }

    fn is_unknown(&self, first: &Value) -> bool {
        match self.unknown {
            Unknown::Never => false,
            Unknown::Null => *first == Value::Null,
            Unknown::Empty => matches!(first, Value::Blob(b) if b.is_empty()),
        }
    }

    /// The condition for the known part of the list, or for the unknown part.
    fn part(&self, unknown: bool) -> Option<String> {
        let first = self.columns[0];
        match (self.unknown, unknown) {
            (Unknown::Never, _) => None,
            (Unknown::Null, false) => Some(format!("{first} IS NOT NULL")),
            (Unknown::Null, true) => Some(format!("{first} IS NULL")),
            (Unknown::Empty, false) => Some(format!("{first} > x''")),
            (Unknown::Empty, true) => Some(format!("{first} = x''")),
        }
    }
}

/// Reads the page of `source` that follows `after`, in `sort` and `order`.
pub fn fetch<T>(
    conn: &Connection,
    source: &Source,
    sort: &Sort,
    order: Order,
    after: Option<Vec<Value>>,
    limit: usize,
    mut map: impl FnMut(&Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<(Vec<T>, Option<String>)> {
    // The row after the page, if there is one, says whether there is a next page.
    let wanted = limit + 1;
    let mut rows: Vec<(T, Vec<Value>)> = Vec::with_capacity(wanted);
    for unknown in parts(sort, after.as_deref()) {
        if rows.len() == wanted {
            break;
        }
        let (sql, params) = part_query(
            source,
            sort,
            order,
            after.as_deref(),
            unknown,
            wanted - rows.len(),
        );
        let mut statement = conn.prepare_cached(&sql)?;
        let mut result = statement.query(params_from_iter(params))?;
        while let Some(row) = result.next()? {
            let width = source.select.len();
            let key = (width..width + sort.columns.len())
                .map(|index| row.get::<_, Value>(index))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.push((map(row)?, key));
        }
    }

    let next = if rows.len() > limit {
        rows.truncate(limit);
        rows.last()
            .map(|(_, key)| cursor::encode(&sort.label(order), key))
    } else {
        None
    };
    Ok((rows.into_iter().map(|(item, _)| item).collect(), next))
}

/// The parts of the list left to read from `after`: `false` for the known part, `true` for
/// the unknown one. A cursor in the unknown part has already passed the known part.
fn parts(sort: &Sort, after: Option<&[Value]>) -> Vec<bool> {
    let starts_unknown = after.is_some_and(|values| sort.is_unknown(&values[0]));
    match (sort.unknown, starts_unknown) {
        (Unknown::Never, _) => vec![false],
        (_, false) => vec![false, true],
        (_, true) => vec![true],
    }
}

/// The query for up to `limit` rows of one part of the list, and its parameters.
fn part_query(
    source: &Source,
    sort: &Sort,
    order: Order,
    after: Option<&[Value]>,
    unknown: bool,
    limit: usize,
) -> (String, Vec<Value>) {
    let (direction, comparison) = match order {
        Order::Asc => ("", ">"),
        Order::Desc => (" DESC", "<"),
    };
    // In the unknown part the first column is the same for every row, so it is left out of
    // the order and the keyset.
    let columns = if unknown {
        &sort.columns[1..]
    } else {
        sort.columns
    };
    // The cursor applies to the part it is in; the part after it starts from the top.
    let starts_unknown = after.is_some_and(|values| sort.is_unknown(&values[0]));
    let cursor = after.filter(|_| unknown == starts_unknown);
    let mut conditions = vec![format!("({})", source.filter)];
    // In the known part a cursor already rules out the unknown rows, except the empty keys
    // below it in a descending list. The redundant bound must be left out: next to the
    // cursor's, it can lead SQLite to skip-scan the index and sort the rest of the list.
    let implied =
        cursor.is_some() && !unknown && !(sort.unknown == Unknown::Empty && order == Order::Desc);
    if !implied {
        conditions.extend(sort.part(unknown));
    }
    let mut params = source.params.clone();
    if let Some(values) = cursor {
        let values = if unknown { &values[1..] } else { values };
        let placeholders = vec!["?"; values.len()].join(", ");
        conditions.push(format!(
            "({}) {comparison} ({placeholders})",
            columns.join(", ")
        ));
        params.extend(values.iter().cloned());
    }
    let order_by = columns
        .iter()
        .map(|column| format!("{column}{direction}"))
        .collect::<Vec<_>>()
        .join(", ");
    // A bound limit keeps the text the same for every page, so the statement cache reuses it.
    params.push(Value::Integer(limit as i64));
    let sql = format!(
        "SELECT {}, {} FROM {} WHERE {} ORDER BY {order_by} LIMIT ?",
        source.select.join(", "),
        sort.columns.join(", "),
        source.from,
        conditions.join(" AND "),
    );
    (sql, params)
}

/// Checks that every query `fetch` makes for `sort` over `source` reads one range of an index
/// and never sorts, so a deep page costs what the first does (`design/database.md` §4). The
/// planner weighs statistics and the cursor's own values, so `source` should hold enough rows
/// for `ANALYZE` to matter, and the cursors are real ones: at the top, among the unknowns, and
/// a tenth, half, and nine tenths of the way in, in both orders.
#[cfg(test)]
pub fn assert_indexed(conn: &Connection, source: &Source, sort: &Sort) -> rusqlite::Result<()> {
    let columns = sort.columns.join(", ");
    let rows: i64 = conn.query_row(
        &format!(
            "SELECT count(*) FROM {} WHERE {}",
            source.from, source.filter
        ),
        params_from_iter(source.params.iter()),
        |row| row.get(0),
    )?;
    let mut cursors = vec![None];
    // In column order, unknown values come first: NULL and the empty key are the smallest.
    for offset in [0, rows / 10, rows / 2, rows * 9 / 10] {
        let sql = format!(
            "SELECT {columns} FROM {} WHERE {} ORDER BY {columns} LIMIT 1 OFFSET {offset}",
            source.from, source.filter
        );
        let values = conn.query_row(&sql, params_from_iter(source.params.iter()), |row| {
            (0..sort.columns.len())
                .map(|index| row.get::<_, Value>(index))
                .collect::<rusqlite::Result<Vec<_>>>()
        })?;
        cursors.push(Some(values));
    }
    for after in &cursors {
        for order in [Order::Asc, Order::Desc] {
            for unknown in parts(sort, after.as_deref()) {
                let (sql, params) = part_query(source, sort, order, after.as_deref(), unknown, 101);
                let plan: Vec<String> = conn
                    .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
                    .query_map(params_from_iter(params), |row| row.get(3))?
                    .collect::<rusqlite::Result<_>>()?;
                let plan = plan.join(" | ");
                assert!(plan.contains("library_id=?"), "{sql}\n{plan}");
                assert!(!plan.contains("TEMP B-TREE"), "{sql}\n{plan}");
            }
        }
    }
    Ok(())
}

/// How many rows `source` holds, for a page's `total`.
pub fn count(conn: &Connection, source: &Source) -> rusqlite::Result<i64> {
    let sql = format!(
        "SELECT count(*) FROM {} WHERE {}",
        source.from, source.filter
    );
    conn.prepare_cached(&sql)?
        .query_row(params_from_iter(source.params.iter()), |row| row.get(0))
}
