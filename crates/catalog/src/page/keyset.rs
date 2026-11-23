//! Keyset paging: the queries behind every browse list (`design/database.md` §4).
//!
//! A page continues from the last row's sort values, never from an offset, so every page costs
//! what the first does. Rows with no sortable value, such as an album with no release date,
//! sort after all the others in both orders (`requirements/conventions.md` §4). Such a list is
//! read in two parts, the known values and then the unknown ones, and each part is one range
//! of the sort's index. Only a sort's first column is treated so; in the columns that break its
//! ties, an unknown value sorts like any other.

use rusqlite::types::Value;
use rusqlite::{Connection, Row, params_from_iter};

use super::{Order, Request};
use crate::{Invalid, cursor};

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

/// One part of a list: the rows whose first sort value is known, or those whose is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Part {
    Known,
    Unknown,
}

/// One way to sort a list.
pub struct Sort {
    /// Names the list and sort in its cursors, such as `albums.name`.
    pub label: &'static str,
    /// The columns to order by. The last is unique, so the order is total, and an index should
    /// cover them after `library_id`.
    pub columns: &'static [&'static str],
    pub unknown: Unknown,
}

/// What a list reads: `SELECT {select} FROM {from} {joins} WHERE {filter}`, with `params` for
/// the `?`s in `from` and `filter`, in order.
pub struct Source {
    /// Columns for the row mapper, which reads them from the front of each row.
    pub select: &'static [&'static str],
    pub from: String,
    /// Tables that only `select` reads, each joined to one row of `from` at most, so a count
    /// leaves them out.
    pub joins: &'static str,
    pub filter: String,
    pub params: Vec<Value>,
    pub total: Total,
    /// Where a count reads when it should not start from these rows.
    pub counting: Option<Counting>,
    /// The table and its alias, such as `("tracks", "t")`, of a list that starts from a lead. A
    /// page then sorts only the IDs and sort keys of the lead's rows under the `LIMIT` and joins
    /// `select` and `joins` to the page's rows after, so it costs the rows' keys, not their
    /// columns. `from` must name the table by that alias and its ID.
    pub back: Option<(&'static str, &'static str)>,
    /// How a page that walks a sort's index checks its rows, and how far it may read.
    pub(crate) walk: Option<Box<Walk>>,
}

/// How much larger a walk's first window is than the rows it is expected to need, and each window
/// than the last. A window is a query of its own, planned afresh for its bounds (measured at
/// 500,000 tracks, about 0.3 ms), and the probe that finds its edge reads the index at about 40 ns
/// a row of it. Larger windows cost fewer plans and more probing; two, three and four times
/// measured alike to within the noise (about 0.5 ms) over tags of 1% to 30% of the library, and
/// four needs the fewest windows.
const WINDOW_GROWTH: i64 = 4;

/// A page that walks a sort's index and checks each row reads rows until a page's worth pass:
/// quick where many rows pass and they are spread through the sort, but all the way to the end of
/// the index where those that pass sort last. So a walk reads only so far, and a page still short
/// then is read from the walk's lead instead, whose cost is the rows that pass, not the index's.
/// How a walk checks its rows says how it reads them.
pub(crate) enum Walk {
    /// In SQL: a tag filter's, whose links are indexed.
    Sql(SqlWalk),
    /// In memory, by ID: a search's matches, which may be hundreds of thousands.
    Ids(IdWalk),
}

/// A walk that checks its rows in SQL reads the index in windows of rows, each larger than the
/// last, and runs over when the windows have covered [`WALK_BUDGET`] rows for each row it asks for.
pub(crate) struct SqlWalk {
    /// The walk's own condition, such as being in the library, without the tags', and its
    /// parameters: what a window's edge is found by.
    pub(crate) filter: String,
    pub(crate) params: Vec<Value>,
    /// How many rows the walk is expected to read for each row it returns, if each row were as
    /// likely as another to carry the smallest group's tags. It sizes the first window.
    pub(crate) per_row: i64,
    /// What the page reads when the walk runs over: the smallest group of tags leading, the
    /// others checked.
    pub(crate) lead: Box<Source>,
}

/// A walk that checks its rows by ID reads the index a chunk at a time and keeps the rows that
/// pass, so it reads about as many rows as it keeps over the share that pass, where they are
/// spread through the sort. Where they are not, such as titles matching a word that starts them,
/// read by name, it runs over after `budget` rows.
pub(crate) struct IdWalk {
    /// Whether the row with this ID passes. A sort's last column is the row's ID.
    pub(crate) keeps: Box<dyn Fn(i64) -> bool>,
    /// About how many rows there are for each one that passes, so each read is enough for the
    /// rest of the page.
    pub(crate) spread: u32,
    /// How many rows the walk reads for one page before it runs over.
    pub(crate) budget: u32,
    /// What the page reads when the walk runs over, made only then, since it costs what the rows
    /// that pass are, which the walk is there to spare.
    pub(crate) lead: Box<MakeLead>,
}

/// Makes the source a walk that checks by ID reads instead when it runs over.
pub(crate) type MakeLead = dyn Fn(&Connection) -> rusqlite::Result<Source>;

/// The rows a page's walk may read for each row it asks for before the page leads instead: twice
/// [`LEAD_SHARE`](crate::tag_filter::LEAD_SHARE). Measured on the generated library of 500,000
/// tracks, the first page of every tag just past the share at which a page walks (0.8% to 1% of the
/// library) read 200 to 233 rows for each it returned, and no later page of any tag read more than
/// 160, so every such walk fits and only one that reads further than any of them leads instead.
pub const WALK_BUDGET: i64 = 256;

/// What a count of a source reads in place of the source's own `from`, `filter` and `params`:
/// the same rows, reached from the smallest of the filter's tag groups through the tag links
/// alone, whether the page itself walks an index or leads from a tag.
pub struct Counting {
    pub from: String,
    pub filter: String,
    pub params: Vec<Value>,
}

/// Where a page's `total` comes from.
pub enum Total {
    /// A count the scanner keeps, read as the source was made.
    Stored(i64),
    /// The source's rows, counted for every page, where no stored count covers its filters.
    Counted,
}

impl Sort {
    /// The label of this sort in `order`, such as `albums.name.asc`, that its cursors carry.
    fn label(&self, order: Order) -> String {
        let order = match order {
            Order::Asc => "asc",
            Order::Desc => "desc",
        };
        format!("{}.{order}", self.label)
    }

    /// Reads a request's cursor, before the query runs.
    pub fn after(&self, cursor: Option<&str>, order: Order) -> Result<Option<Vec<Value>>, Invalid> {
        cursor.map(|text| cursor::decode(text, &self.label(order), self.columns.len())).transpose()
    }

    /// The part of the list a row whose sort values start with `first` is in.
    fn part_of(&self, first: &Value) -> Part {
        match (self.unknown, first) {
            (Unknown::Null, Value::Null) => Part::Unknown,
            (Unknown::Empty, Value::Blob(b)) if b.is_empty() => Part::Unknown,
            _ => Part::Known,
        }
    }

    /// The condition for `part` of the list, if the list has parts.
    fn condition(&self, part: Part) -> Option<String> {
        let first = self.columns[0];
        match (self.unknown, part) {
            (Unknown::Never, _) => None,
            (Unknown::Null, Part::Known) => Some(format!("{first} IS NOT NULL")),
            (Unknown::Null, Part::Unknown) => Some(format!("{first} IS NULL")),
            (Unknown::Empty, Part::Known) => Some(format!("{first} > x''")),
            (Unknown::Empty, Part::Unknown) => Some(format!("{first} = x''")),
        }
    }
}

/// Reads the page of `source` that `request` asks for. A source with a [`Walk`] walks its index,
/// and reads from the walk's lead instead if it runs over.
pub fn fetch<T>(
    conn: &Connection,
    source: &Source,
    request: &Request,
    mut map: impl FnMut(&Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<(Vec<T>, Option<String>)> {
    if let Some(walk) = &source.walk {
        if let Some(page) = self::walk(conn, source, walk, request, &mut map)? {
            return Ok(page);
        }
        let made;
        let lead = match &**walk {
            Walk::Sql(walk) => &*walk.lead,
            Walk::Ids(walk) => {
                made = (walk.lead)(conn)?;
                &made
            }
        };
        let page = read_page(conn, lead, None, request, &mut map)?;
        return Ok(page.expect("a page read with no walk is always finished"));
    }
    Ok(read_page(conn, source, None, request, &mut map)?
        .expect("a page read with no walk is finished"))
}

/// The page of `source` that `request` asks for, walking its index as `walk` checks its rows, and
/// `None` if the walk ran over without filling the page.
pub(super) fn walk<T>(
    conn: &Connection,
    source: &Source,
    walk: &Walk,
    request: &Request,
    map: &mut impl FnMut(&Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<Option<(Vec<T>, Option<String>)>> {
    match walk {
        Walk::Sql(walk) => read_page(conn, source, Some(walk), request, map),
        Walk::Ids(walk) => read_chunks(conn, source, walk, request, map),
    }
}

/// A page's rows so far, each with its sort values.
type Rows<T> = Vec<(T, Vec<Value>)>;

/// The sort values of `row`, which follow the columns `source` selects.
fn sort_values(row: &Row, source: &Source, sort: &Sort) -> rusqlite::Result<Vec<Value>> {
    let width = source.select.len();
    (width..width + sort.columns.len()).map(|index| row.get::<_, Value>(index)).collect()
}

/// The page `rows` make, read for `request` with `left` rows more still wanted: the row after
/// the page, read when there is one, gives the page a cursor to it.
fn finish<T>(mut rows: Rows<T>, left: u32, request: &Request) -> (Vec<T>, Option<String>) {
    let next = if left == 0 {
        rows.pop();
        rows.last().map(|(_, key)| cursor::encode(&request.sort.label(request.order), key))
    } else {
        None
    };
    (rows.into_iter().map(|(item, _)| item).collect(), next)
}

/// The most rows a walk that checks by ID reads at once, which bounds what one read holds in
/// memory.
const MAX_CHUNK: u32 = 10_000;

/// The page of `source` that `request` asks for, of the rows `checks` passes, read a chunk of the
/// index at a time, and `None` if the walk ran over its budget without filling the page.
fn read_chunks<T>(
    conn: &Connection,
    source: &Source,
    checks: &IdWalk,
    request: &Request,
    map: &mut impl FnMut(&Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<Option<(Vec<T>, Option<String>)>> {
    // The row after the page, if there is one, says whether there is a next page.
    let mut left = request.limit + 1;
    let mut walked: u32 = 0;
    let mut rows: Rows<T> = Vec::new();
    let after = request.after.as_deref();
    // A sort's last column is the row's ID, which is all a row the walk passes over needs read.
    let id_column = source.select.len() + request.sort.columns.len() - 1;
    for &part in parts(request.sort, after) {
        // Where the next read of this part starts: the cursor, then the last row read.
        let mut from = after.map(<[Value]>::to_vec);
        while left > 0 {
            // Half again what the share predicts, so a page rarely needs two reads.
            let chunked = left.saturating_mul(checks.spread).saturating_mul(3) / 2;
            let want = chunked.clamp(left, MAX_CHUNK.max(left));
            let (sql, params) =
                part_query(source, request.sort, request.order, from.as_deref(), part, want, None);
            let mut statement = conn.prepare_cached(&sql)?;
            let mut result = statement.query(params_from_iter(params))?;
            let (mut read, mut last) = (0, None);
            while left > 0
                && let Some(row) = result.next()?
            {
                read += 1;
                let kept = (checks.keeps)(row.get(id_column)?);
                // A full read's last row is where the next one starts, kept or not.
                let edge = read == want;
                if !kept && !edge {
                    continue;
                }
                let key = sort_values(row, source, request.sort)?;
                if !kept {
                    last = Some(key);
                    continue;
                }
                if edge {
                    last = Some(key.clone());
                }
                rows.push((map(row)?, key));
                left -= 1;
            }
            // A short read is the end of the part; a full one continues after its last row.
            if read < want {
                break;
            }
            walked = walked.saturating_add(read);
            if left > 0 && walked >= checks.budget {
                return Ok(None);
            }
            from = last;
        }
    }
    Ok(Some(finish(rows, left, request)))
}

/// The page of `source` that `request` asks for, read a window of the index at a time if `walk`
/// is given, and `None` if the walk read its whole budget without filling the page.
fn read_page<T>(
    conn: &Connection,
    source: &Source,
    walk: Option<&SqlWalk>,
    request: &Request,
    map: &mut impl FnMut(&Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<Option<(Vec<T>, Option<String>)>> {
    // The row after the page, if there is one, says whether there is a next page.
    let mut left = request.limit + 1;
    let budget = i64::from(left) * WALK_BUDGET;
    let mut spent = 0;
    let mut rows: Rows<T> = Vec::new();
    let after = request.after.as_deref();
    for &part in parts(request.sort, after) {
        if left == 0 {
            break;
        }
        let Some(walk) = walk else {
            read(conn, source, request, after, part, None, &mut left, &mut rows, map)?;
            continue;
        };
        let mut from = after.map(<[Value]>::to_vec);
        let mut window = (i64::from(left) * walk.per_row * WINDOW_GROWTH).min(budget);
        loop {
            let edge = edge(conn, source, walk, request, from.as_deref(), part, window)?;
            let until = edge.as_deref();
            read(conn, source, request, from.as_deref(), part, until, &mut left, &mut rows, map)?;
            let Some(edge) = edge else { break };
            if left == 0 {
                break;
            }
            spent += window;
            if spent >= budget {
                return Ok(None);
            }
            (from, window) = (Some(edge), (window * WINDOW_GROWTH).min(budget - spent));
        }
    }
    Ok(Some(finish(rows, left, request)))
}

/// Reads one part of the list after `after`, up to the sort values `until` if given, into `rows`,
/// until `left` rows more are wanted no more.
#[expect(clippy::too_many_arguments, reason = "one read's place in a page")]
fn read<T>(
    conn: &Connection,
    source: &Source,
    request: &Request,
    after: Option<&[Value]>,
    part: Part,
    until: Option<&[Value]>,
    left: &mut u32,
    rows: &mut Rows<T>,
    map: &mut impl FnMut(&Row) -> rusqlite::Result<T>,
) -> rusqlite::Result<()> {
    let (sql, params) = part_query(source, request.sort, request.order, after, part, *left, until);
    let mut statement = conn.prepare_cached(&sql)?;
    let mut result = statement.query(params_from_iter(params))?;
    while let Some(row) = result.next()? {
        let key = sort_values(row, source, request.sort)?;
        rows.push((map(row)?, key));
        *left -= 1;
    }
    Ok(())
}

/// The query for the sort values of the row `window` rows on from `after` in `part` of the walk's
/// rows (the last of a window of that many), and its parameters. The rows are counted before the
/// tags, from the index alone.
fn edge_query(
    source: &Source,
    walk: &SqlWalk,
    sort: &Sort,
    order: Order,
    after: Option<&[Value]>,
    part: Part,
    window: i64,
) -> (String, Vec<Value>) {
    let (conditions, mut params, order_by) =
        range(&walk.filter, &walk.params, sort, order, after, part, None);
    let sql = format!(
        "SELECT {} FROM {} WHERE {conditions} ORDER BY {order_by} LIMIT 1 OFFSET ?",
        sort.columns.join(", "),
        source.from
    );
    params.push(Value::from(window - 1));
    (sql, params)
}

/// The sort values of the last row of a window of `window` rows on from `after` in `part` of the
/// walk's rows, or `None` if the part ends before that. A walk reads up to it, then from it, so no
/// row is read twice.
fn edge(
    conn: &Connection,
    source: &Source,
    walk: &SqlWalk,
    request: &Request,
    after: Option<&[Value]>,
    part: Part,
    window: i64,
) -> rusqlite::Result<Option<Vec<Value>>> {
    let (sql, params) = edge_query(source, walk, request.sort, request.order, after, part, window);
    let mut statement = conn.prepare_cached(&sql)?;
    let mut result = statement.query(params_from_iter(params))?;
    let Some(row) = result.next()? else { return Ok(None) };
    (0..request.sort.columns.len()).map(|index| row.get(index)).collect::<Result<_, _>>().map(Some)
}

/// The parts of the list left to read from `after`, in order. A cursor in the unknown part has
/// already passed the known part.
fn parts(sort: &Sort, after: Option<&[Value]>) -> &'static [Part] {
    let start = after.map_or(Part::Known, |values| sort.part_of(&values[0]));
    match (sort.unknown, start) {
        (Unknown::Never, _) => &[Part::Known],
        (_, Part::Known) => &[Part::Known, Part::Unknown],
        (_, Part::Unknown) => &[Part::Unknown],
    }
}

/// The conditions, their parameters and the order of one part of a list of the rows `filter`
/// picks, from after the sort values `after` up to and including `until`.
fn range(
    filter: &str,
    filter_params: &[Value],
    sort: &Sort,
    order: Order,
    after: Option<&[Value]>,
    part: Part,
    until: Option<&[Value]>,
) -> (String, Vec<Value>, String) {
    let (direction, ahead, behind) = match order {
        Order::Asc => ("", ">", "<="),
        Order::Desc => (" DESC", "<", ">="),
    };
    // In the unknown part the first column is the same for every row, so it is left out of
    // the order and the keyset.
    let unknown = part == Part::Unknown;
    let columns = if unknown { &sort.columns[1..] } else { sort.columns };
    // The cursor applies to the part it is in; the part after it starts from the top.
    let cursor = after.filter(|values| sort.part_of(&values[0]) == part);
    let mut conditions = vec![format!("({filter})")];
    // In the known part a cursor already rules out the unknown rows, except the empty keys
    // below it in a descending list. The redundant bound must be left out: next to the
    // cursor's, it can lead SQLite to skip-scan the index and sort the rest of the list.
    let implied =
        cursor.is_some() && !unknown && !(sort.unknown == Unknown::Empty && order == Order::Desc);
    if !implied {
        conditions.extend(sort.condition(part));
    }
    let mut params = filter_params.to_vec();
    let mut bound = |values: &[Value], comparison: &str| {
        let values = if unknown { &values[1..] } else { values };
        let placeholders = vec!["?"; values.len()].join(", ");
        conditions.push(format!("({}) {comparison} ({placeholders})", columns.join(", ")));
        params.extend(values.iter().cloned());
    };
    if let Some(values) = cursor {
        bound(values, ahead);
    }
    if let Some(values) = until {
        bound(values, behind);
    }
    let order_by =
        columns.iter().map(|column| format!("{column}{direction}")).collect::<Vec<_>>().join(", ");
    (conditions.join(" AND "), params, order_by)
}

/// The query for up to `limit` rows of one part of the list, from after `after` up to and
/// including `until` if given, and its parameters. The sort's columns follow the source's in each
/// row, as a page's cursor needs.
pub(super) fn part_query(
    source: &Source,
    sort: &Sort,
    order: Order,
    after: Option<&[Value]>,
    part: Part,
    limit: u32,
    until: Option<&[Value]>,
) -> (String, Vec<Value>) {
    let (conditions, mut params, order_by) =
        range(&source.filter, &source.params, sort, order, after, part, until);
    // A bound limit keeps the text the same for every page, so the statement cache reuses it.
    params.push(Value::from(limit));
    let sql = match source.back {
        None => format!(
            "SELECT {}, {} FROM {} {} WHERE {conditions} ORDER BY {order_by} LIMIT ?",
            source.select.join(", "),
            sort.columns.join(", "),
            source.from,
            source.joins,
        ),
        // The page's rows are picked by their sort keys alone, then the columns are joined to
        // them: a lead's rows are all sorted, so what each carries is what the sort moves.
        Some((table, alias)) => {
            let keys = |prefix: &str| {
                (0..sort.columns.len()).map(|at| format!("{prefix}k{at}")).collect::<Vec<_>>()
            };
            let aliased =
                sort.columns.iter().zip(keys("")).map(|(column, key)| format!("{column} AS {key}"));
            let skip = usize::from(part == Part::Unknown);
            let direction = if order == Order::Asc { "" } else { " DESC" };
            let outer: Vec<String> =
                keys("page.").iter().skip(skip).map(|key| format!("{key}{direction}")).collect();
            format!(
                "SELECT {select}, {keys} FROM (SELECT {alias}.id AS id, {aliased} FROM {from} \
                 WHERE {conditions} ORDER BY {order_by} LIMIT ?) page \
                 CROSS JOIN {table} {alias} ON {alias}.id = page.id {joins} ORDER BY {outer}",
                select = source.select.join(", "),
                keys = keys("page.").join(", "),
                aliased = aliased.collect::<Vec<_>>().join(", "),
                from = source.from,
                joins = source.joins,
                outer = outer.join(", "),
            )
        }
    };
    (sql, params)
}

/// The first column of every row of `source` as an ID, in the order of `sort` and `order`: the
/// whole list at once, which is what `fetch` pages through to its end. Each part of the list is
/// one statement over the same range of the sort's index, with no limit. The source's first
/// column must be the ID.
pub fn ids(
    conn: &Connection,
    source: &Source,
    sort: &Sort,
    order: Order,
) -> rusqlite::Result<Vec<u64>> {
    let mut ids = Vec::new();
    for &part in parts(sort, None) {
        let (sql, params) = part_query(source, sort, order, None, part, u32::MAX, None);
        let mut statement = conn.prepare_cached(&sql)?;
        let mut rows = statement.query(params_from_iter(params))?;
        while let Some(row) = rows.next()? {
            ids.push(row.get::<_, i64>(0)?.cast_unsigned());
        }
    }
    Ok(ids)
}

/// Checks that every query `fetch` makes for `sort` over `source` reads one range of an index
/// and never sorts, so a deep page costs what the first does (`design/database.md` §4). The
/// planner weighs statistics and the cursor's own values, so `source` should hold enough rows
/// for `ANALYZE` to matter, and the cursors are real ones: at the top, among the unknowns, and
/// a tenth, half, and nine tenths of the way in, in both orders.
#[cfg(any(test, feature = "testing"))]
pub fn assert_indexed(conn: &Connection, source: &Source, sort: &Sort) -> rusqlite::Result<()> {
    let columns = sort.columns.join(", ");
    let rows: i64 = conn.query_row(
        &format!("SELECT count(*) FROM {} WHERE {}", source.from, source.filter),
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
            for &part in parts(sort, after.as_deref()) {
                let (sql, params) =
                    part_query(source, sort, order, after.as_deref(), part, 101, None);
                assert_one_range(conn, &sql, params)?;
                // A walk's windows are ranges of the same index: the query that finds a window's
                // edge, and the page's query held to that edge. A walk by ID reads only chunks of
                // the part's query.
                let Some(Walk::Sql(walk)) = source.walk.as_deref() else { continue };
                let (sql, params) =
                    edge_query(source, walk, sort, order, after.as_deref(), part, 50);
                assert_one_range(conn, &sql, params.clone())?;
                let edge: Option<Vec<Value>> = conn
                    .prepare(&sql)?
                    .query_map(params_from_iter(params), |row| {
                        (0..sort.columns.len()).map(|index| row.get(index)).collect()
                    })?
                    .next()
                    .transpose()?;
                let Some(edge) = edge else { continue };
                let (sql, params) =
                    part_query(source, sort, order, after.as_deref(), part, 101, Some(&edge));
                assert_one_range(conn, &sql, params)?;
            }
        }
    }
    Ok(())
}

/// Checks that `sql` reads one range of an index and never sorts.
#[cfg(any(test, feature = "testing"))]
fn assert_one_range(conn: &Connection, sql: &str, params: Vec<Value>) -> rusqlite::Result<()> {
    // Each step's parent, and what it does. The page's own steps have parent 0; a column's
    // subquery, such as a track's credits, nests below them.
    let steps: Vec<(i64, String)> = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
        .query_map(params_from_iter(params), |row| Ok((row.get(1)?, row.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let plan = steps.iter().map(|(_, step)| step.as_str()).collect::<Vec<_>>();
    let plan = plan.join(" | ");
    let page: Vec<&str> =
        steps.iter().filter(|(parent, _)| *parent == 0).map(|(_, step)| step.as_str()).collect();
    // A search of an index on its leading columns, not a skip-scan past them, and no sort of the
    // page. Every subquery searches by a key too.
    assert!(page[0].starts_with("SEARCH"), "{sql}\n{plan}");
    assert!(!page.iter().any(|step| step.contains("TEMP B-TREE")), "{sql}\n{plan}");
    assert!(!plan.contains("ANY("), "{sql}\n{plan}");
    assert!(!plan.contains("SCAN "), "{sql}\n{plan}");
    Ok(())
}

/// How many rows `source` holds, for a page's `total`, as the query and parameters that count
/// them.
pub(super) fn count_query(source: &Source) -> (String, &[Value]) {
    let (from, filter, params) = match &source.counting {
        Some(Counting { from, filter, params }) => (from, filter, params),
        None => (&source.from, &source.filter, &source.params),
    };
    (format!("SELECT count(*) FROM {from} WHERE {filter}"), params)
}

/// Counts the rows of `source`.
pub fn count(conn: &Connection, source: &Source) -> rusqlite::Result<i64> {
    let (sql, params) = count_query(source);
    conn.prepare_cached(&sql)?.query_row(params_from_iter(params.iter()), |row| row.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_reads_from_the_counting_source_when_there_is_one() {
        let conn = Connection::open_in_memory().unwrap();
        let source = |counting| Source {
            select: &["n"],
            from: "(SELECT 1 AS n UNION ALL SELECT 2 UNION ALL SELECT 3) t".to_owned(),
            joins: "",
            filter: "n > ?".to_owned(),
            params: vec![Value::from(0)],
            total: Total::Counted,
            counting,
            back: None,
            walk: None,
        };
        assert_eq!(count(&conn, &source(None)).unwrap(), 3);
        let counting = Counting {
            from: "(SELECT 1 AS n UNION ALL SELECT 2) t".to_owned(),
            filter: "n > ?".to_owned(),
            params: vec![Value::from(1)],
        };
        assert_eq!(count(&conn, &source(Some(counting))).unwrap(), 1);
    }

    /// A list of `t`, sorted by `k` and then ID, as a list's sort is.
    static BY_K: Sort = Sort { label: "t.k", columns: &["t.k", "t.id"], unknown: Unknown::Never };

    /// A thousand rows whose `k` puts them in an order of their own, apart from their IDs.
    fn rows() -> (Connection, Source) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, k INTEGER NOT NULL);
             CREATE INDEX t_k ON t (k, id);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000)
             INSERT INTO t SELECT i, i * 37 % 101 FROM n;",
        )
        .unwrap();
        let source = Source {
            select: &["t.id"],
            from: "t".to_owned(),
            joins: "",
            filter: "1".to_owned(),
            params: Vec::new(),
            total: Total::Stored(1000),
            counting: None,
            back: None,
            walk: None,
        };
        (conn, source)
    }

    /// The IDs of `t` in `BY_K`'s order that `keeps` keeps: what a walk should find.
    fn sorted(conn: &Connection, keeps: impl Fn(i64) -> bool) -> Vec<i64> {
        let mut statement = conn.prepare("SELECT id FROM t ORDER BY k, id").unwrap();
        let ids = statement.query_map([], |row| row.get(0)).unwrap();
        ids.map(Result::unwrap).filter(|&id| keeps(id)).collect()
    }

    /// A walk that keeps every seventh ID, `budget` rows long.
    fn sevenths(budget: u32) -> IdWalk {
        IdWalk {
            keeps: Box::new(|id| id % 7 == 0),
            // Too few, so each page takes several chunks.
            spread: 1,
            budget,
            lead: Box::new(|_| unreachable!("the walk is read alone")),
        }
    }

    #[test]
    fn a_walk_by_id_pages_through_its_rows_a_chunk_at_a_time() {
        let (conn, source) = rows();
        // The whole table, so a walk that went wrong ends.
        let walk = sevenths(1000);
        let (mut found, mut cursor) = (Vec::new(), None);
        loop {
            let request = Request::new(&BY_K, Order::Asc, cursor, Some(10)).unwrap();
            let map = &mut |row: &Row| row.get::<_, i64>(0);
            let (ids, next) = read_chunks(&conn, &source, &walk, &request, map).unwrap().unwrap();
            assert!(ids.len() == 10 || next.is_none(), "a full page but the last");
            found.extend(ids);
            cursor = next;
            if cursor.is_none() {
                break;
            }
        }
        // Each page takes several chunks, each continuing after the last.
        assert_eq!(found, sorted(&conn, |id| id % 7 == 0));
    }

    #[test]
    fn a_walk_by_id_runs_over_once_it_passes_its_budget() {
        let (conn, source) = rows();
        let request = Request::new(&BY_K, Order::Asc, None, Some(10)).unwrap();
        let read = |walk: &IdWalk| {
            let map = &mut |row: &Row| row.get::<_, i64>(0);
            read_chunks(&conn, &source, walk, &request, map).unwrap()
        };
        // The page and the row after it, the first eleven sevenths, sort within the first 74
        // rows, which a walk of 80 reaches and one of 64 does not.
        let page = read(&sevenths(80)).expect("the page fits the budget");
        assert_eq!(page.0, sorted(&conn, |id| id % 7 == 0)[..10]);
        assert_eq!(read(&sevenths(64)), None, "64 rows read and the page still short");
        // Rows that all sort last are passed over until the budget runs out, however big.
        let last = IdWalk { keeps: Box::new(|id| id * 37 % 101 == 100), ..sevenths(500) };
        assert_eq!(read(&last), None);
    }
}
