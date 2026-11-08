//! An entity's device rows: what the library change feed carries for one changed track, album,
//! artist or tag, and how a device writes them (`design/general.md` §3).
//!
//! The rows are the entity's own table row and the join rows that hang off it (a track's
//! artists and tags, an album's artists, credits, discs and tags), with exactly the columns of
//! the device schema, so nothing the device schema leaves out can leave the server
//! (`requirements/general.md` §3.7). Images and tag types are not entities of their own: an
//! `images` row travels in the rows of the track, album or artist that shows it, and a
//! `tag_types` row in those of its tag. Embedded summaries are not carried: the device joins
//! them from its own rows.
//!
//! On the wire a row is an object keyed by the device schema's column names. As in the API, an
//! ID is a decimal string, since a random 63-bit ID does not survive JSON in JavaScript, and a
//! blob is base64. A key that is not a column is ignored on `apply`, so a server that adds a
//! column never breaks a device that has not learned it; a column a row lacks is refused.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::OnceLock;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use rusqlite::types::{Value as Sql, ValueRef};
use rusqlite::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A row as it travels: its cells by column name.
pub type Row = Map<String, Value>;

/// What a change names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Artist,
    Album,
    Track,
    Tag,
}

/// A table of an entity's rows.
struct Part {
    table: &'static str,
    /// The condition selecting the entity's rows, with its ID as `?1` and, on the server, its
    /// library as `?2`.
    by: &'static str,
    /// The column the rows are listed by.
    order: &'static str,
    /// Rows other entities share, such as an image: written over on `apply`, never deleted.
    shared: bool,
}

/// A table whose rows belong to one entity: `apply` replaces them and `delete` removes them.
const fn own(table: &'static str, by: &'static str, order: &'static str) -> Part {
    Part { table, by, order, shared: false }
}

/// A table whose rows other entities may share, so `delete` leaves them. `by` selects them
/// through the entity's own row, and they have no order.
const fn shared(table: &'static str, by: &'static str) -> Part {
    Part { table, by, order: "1", shared: true }
}

const ARTIST: &[Part] = &[
    own("artists", "id = ?1", "id"),
    own("artist_tags", "artist_id = ?1", "tag_id"),
    shared("images", "id = (SELECT image_id FROM artists WHERE id = ?1 AND library_id = ?2)"),
];

const ALBUM: &[Part] = &[
    own("albums", "id = ?1", "id"),
    own("album_artists", "album_id = ?1", "position"),
    own("album_credits", "album_id = ?1", "artist_id"),
    own("album_discs", "album_id = ?1", "disc_number"),
    own("album_tags", "album_id = ?1", "tag_id"),
    shared("images", "id = (SELECT image_id FROM albums WHERE id = ?1 AND library_id = ?2)"),
];

const TRACK: &[Part] = &[
    own("tracks", "id = ?1", "id"),
    own("track_artists", "track_id = ?1", "position"),
    own("track_tags", "track_id = ?1", "tag_id"),
    shared("images", "id = (SELECT image_id FROM tracks WHERE id = ?1 AND library_id = ?2)"),
];

const TAG: &[Part] = &[
    own("tags", "id = ?1", "id"),
    shared("tag_types", "key = (SELECT type FROM tags WHERE id = ?1 AND library_id = ?2)"),
];

impl Kind {
    /// The entity's tables, the one row that is the entity first. The images an entity shows
    /// and a tag's type travel with it.
    fn parts(self) -> &'static [Part] {
        match self {
            Kind::Artist => ARTIST,
            Kind::Album => ALBUM,
            Kind::Track => TRACK,
            Kind::Tag => TAG,
        }
    }
}

/// One entity's device rows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rows {
    pub kind: Kind,
    /// The entity's ID as the API spells it.
    pub id: String,
    /// Each table's rows, in order. A table with none is left out.
    pub tables: BTreeMap<String, Vec<Row>>,
}

/// Why rows could not be applied.
#[derive(Debug)]
pub enum Error {
    /// The rows do not fit the device schema.
    Invalid(&'static str),
    Sql(rusqlite::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid(why) => write!(f, "invalid rows: {why}"),
            Error::Sql(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Error {
        Error::Sql(error)
    }
}

struct Column {
    name: String,
    ty: String,
}

/// The device schema's columns of `table`, in order.
fn columns(table: &str) -> &[Column] {
    static COLUMNS: OnceLock<HashMap<String, Vec<Column>>> = OnceLock::new();
    let all = COLUMNS.get_or_init(|| {
        let read = || -> rusqlite::Result<HashMap<String, Vec<Column>>> {
            let conn = Connection::open_in_memory()?;
            crate::create_schema(&conn)?;
            let mut all: HashMap<String, Vec<Column>> = HashMap::new();
            let mut stmt = conn.prepare(
                "SELECT m.name, p.name, p.type FROM sqlite_schema AS m, \
                 pragma_table_info(m.name) AS p WHERE m.type = 'table' ORDER BY m.name, p.cid",
            )?;
            for row in stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
                let (table, name, ty): (String, String, String) = row?;
                all.entry(table).or_default().push(Column { name, ty });
            }
            Ok(all)
        };
        read().expect("the device schema builds in memory")
    });
    all.get(table).map_or(&[], Vec::as_slice)
}

/// Whether the column holds an ID, which travels as a string.
fn is_id(column: &str) -> bool {
    column == "id" || column.ends_with("_id")
}

/// Reads an ID in its one canonical spelling, as the API does.
fn parse_id(id: &str) -> Option<i64> {
    id.parse::<i64>().ok().filter(|n| n.to_string() == id)
}

/// Reads the rows of the entity `id` of `kind` in `library`, or `None` if there is no such
/// entity there. `conn` may be the server's database or a device's: both have the device
/// columns, and only those are read.
pub fn export(
    conn: &Connection,
    library: i64,
    kind: Kind,
    id: &str,
) -> rusqlite::Result<Option<Rows>> {
    let Some(entity) = parse_id(id) else { return Ok(None) };
    let params = [entity, library];
    let mut tables = BTreeMap::new();
    for Part { table, by, order, .. } in kind.parts() {
        let cols = columns(table);
        let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
        let scope = if *table == "tag_types" { "" } else { " AND library_id = ?2" };
        let sql =
            format!("SELECT {} FROM {table} WHERE {by}{scope} ORDER BY {order}", names.join(", "));
        let rows = conn
            .prepare_cached(&sql)?
            .query_map(params, |row| {
                cols.iter()
                    .enumerate()
                    .map(|(i, col)| {
                        row.get_ref(i).map(|cell| (col.name.clone(), to_json(col, cell)))
                    })
                    .collect::<rusqlite::Result<Row>>()
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.is_empty() && tables.is_empty() {
            return Ok(None);
        }
        if !rows.is_empty() {
            tables.insert((*table).to_owned(), rows);
        }
    }
    Ok(Some(Rows { kind, id: id.to_owned(), tables }))
}

/// A cell as it travels.
fn to_json(col: &Column, cell: ValueRef) -> Value {
    match cell {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) if is_id(&col.name) => Value::String(n.to_string()),
        ValueRef::Integer(n) => n.into(),
        ValueRef::Real(f) => f.into(),
        ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => Value::String(BASE64.encode(b)),
    }
}

/// A cell as the device column stores it.
fn to_sql(col: &Column, cell: &Value) -> Result<Sql, Error> {
    Ok(match (col.ty.as_str(), cell) {
        (_, Value::Null) => Sql::Null,
        ("INTEGER", Value::String(s)) if is_id(&col.name) => {
            Sql::Integer(parse_id(s).ok_or(Error::Invalid("an ID is not an ID"))?)
        }
        ("INTEGER", Value::Number(n)) if !is_id(&col.name) => {
            Sql::Integer(n.as_i64().ok_or(Error::Invalid("a number is not an integer"))?)
        }
        ("REAL", Value::Number(n)) => Sql::Real(n.as_f64().unwrap_or_default()),
        ("TEXT", Value::String(s)) => Sql::Text(s.clone()),
        ("BLOB", Value::String(s)) => {
            Sql::Blob(BASE64.decode(s).map_err(|_| Error::Invalid("a blob is not base64"))?)
        }
        _ => return Err(Error::Invalid("a cell does not fit its column")),
    })
}

/// Replaces the entity's rows on the device with `rows`, removing the join rows that are no
/// longer there, and writes over the images and tag type that came with it. It makes several
/// statements, so a caller that must not leave a half-applied entity runs it in a transaction,
/// as a catch-up does. On an error some of the entity's rows may have changed.
pub fn apply(conn: &Connection, rows: &Rows) -> Result<(), Error> {
    let parts = rows.kind.parts();
    if rows.tables.keys().any(|name| parts.iter().all(|part| part.table != name)) {
        return Err(Error::Invalid("a table is not one of the entity's"));
    }
    let id = parse_id(&rows.id).ok_or(Error::Invalid("the entity's ID is not an ID"))?;
    delete(conn, rows.kind, id)?;
    for Part { table, shared, .. } in parts {
        let Some(list) = rows.tables.get(*table) else { continue };
        let cols = columns(table);
        let names: Vec<&str> = cols.iter().map(|c| c.name.as_str()).collect();
        let marks = vec!["?"; cols.len()].join(", ");
        let verb = if *shared { "INSERT OR REPLACE" } else { "INSERT" };
        let mut stmt = conn.prepare_cached(&format!(
            "{verb} INTO {table} ({}) VALUES ({marks})",
            names.join(", ")
        ))?;
        for row in list {
            let cells = cols
                .iter()
                .map(|col| {
                    let cell = row.get(&col.name).ok_or(Error::Invalid("a row lacks a column"))?;
                    to_sql(col, cell)
                })
                .collect::<Result<Vec<_>, _>>()?;
            stmt.execute(params_from_iter(cells))?;
        }
    }
    Ok(())
}

/// Removes the entity's own rows from the device. The images and tag type it showed stay, as
/// other entities may show them. An entity that is not there is no change.
pub fn delete(conn: &Connection, kind: Kind, id: i64) -> rusqlite::Result<()> {
    for Part { table, by, .. } in kind.parts().iter().filter(|part| !part.shared) {
        conn.prepare_cached(&format!("DELETE FROM {table} WHERE {by}"))?.execute([id])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The tag's row, with `edit` applied to its cells.
    fn tag_row(edit: impl FnOnce(&mut Row)) -> Row {
        let mut row = json!({
            "id": "5", "library_id": "1", "type": "genre", "name": "Rock", "sort_key": "AA==",
            "track_count": 0, "album_count": 0, "artist_count": 0,
        })
        .as_object()
        .unwrap()
        .clone();
        edit(&mut row);
        row
    }

    /// Rows of the tag `id` with the given tables.
    fn tag(id: &str, tables: Value) -> Rows {
        Rows { kind: Kind::Tag, id: id.into(), tables: serde_json::from_value(tables).unwrap() }
    }

    /// A device database with the schema.
    fn device() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::create_schema(&conn).unwrap();
        conn
    }

    /// The number of rows in `table`.
    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn rows_that_do_not_fit_the_device_schema_are_refused() {
        let conn = device();
        let with = |edit: fn(&mut Row)| tag("5", json!({ "tags": [tag_row(edit)] }));
        for bad in [
            tag("5", json!({ "tracks": [] })),
            with(|r| drop(r.remove("sort_key"))),
            with(|r| drop(r.insert("id".into(), json!(5)))),
            with(|r| drop(r.insert("track_count".into(), json!("0")))),
            with(|r| drop(r.insert("sort_key".into(), json!("not base64!")))),
            tag("05", json!({})),
        ] {
            assert!(matches!(apply(&conn, &bad), Err(Error::Invalid(_))), "{bad:?}");
        }
        assert_eq!(count(&conn, "tags"), 0);
    }

    #[test]
    fn a_key_that_is_no_column_is_ignored() {
        let conn = device();
        let row = tag_row(|r| drop(r.insert("added_later".into(), json!({ "any": [1] }))));
        apply(&conn, &tag("5", json!({ "tags": [row] }))).unwrap();
        assert_eq!(count(&conn, "tags"), 1);
    }

    #[test]
    fn a_tags_type_is_written_over_and_outlives_the_tag() {
        let conn = device();
        let rows = |name: &str| {
            tag(
                "5",
                json!({
                    "tags": [tag_row(|_| {})],
                    "tag_types": [{ "key": "genre", "name": name }],
                }),
            )
        };
        apply(&conn, &rows("Genre")).unwrap();
        apply(&conn, &rows("Style")).unwrap();
        let name: String = conn
            .query_row("SELECT name FROM tag_types WHERE key = 'genre'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Style");
        delete(&conn, Kind::Tag, 5).unwrap();
        assert_eq!((count(&conn, "tags"), count(&conn, "tag_types")), (0, 1));
    }
}
