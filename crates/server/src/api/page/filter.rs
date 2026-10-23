//! Filtered lists: the rows a page starts from, and where its total comes from
//! (`design/database.md` §4).

use rusqlite::Connection;
use rusqlite::types::Value;

use super::{Source, Total, library_count};

/// A list's table, as every page of it reads it.
pub struct Table {
    /// The table, such as `tracks`.
    pub name: &'static str,
    /// What the list's columns call the table, such as `t`.
    pub alias: &'static str,
    /// Columns for the row mapper, which reads them from the front of each row.
    pub select: &'static [&'static str],
    /// Tables that only `select` reads, each joined to one row of the table at most.
    pub joins: &'static str,
    /// The library's stored count of the table's rows, such as `track_count`.
    pub count: &'static str,
}

/// Rows a page starts from and sorts, such as an artist's tracks.
pub struct Lead {
    /// Their IDs, in a column named `id`, with a `?` for each of `params`.
    pub ids: String,
    pub params: Vec<Value>,
    /// How many there are, from the counts the scanner keeps.
    pub size: i64,
}

/// A condition each row of a page meets, such as being on one album.
pub struct Check {
    /// The condition on a row of the table, with a `?` for each of `params`.
    pub condition: String,
    pub params: Vec<Value>,
}

impl Table {
    /// The rows of the table in `library`, or those of `lead`, that meet every one of `checks`.
    /// The total is the library's stored count, or the lead's size, and is counted only where
    /// there are checks. `library` comes from the caller's `Scope`, so the caller reaches it.
    pub fn source(
        &self,
        conn: &Connection,
        library: i64,
        lead: Option<Lead>,
        checks: Vec<Check>,
    ) -> rusqlite::Result<Source> {
        let (name, alias) = (self.name, self.alias);
        let total = match (&lead, checks.is_empty()) {
            (None, true) => Total::Stored(library_count(conn, library, self.count)?),
            (Some(lead), true) => Total::Stored(lead.size),
            (_, false) => Total::Counted,
        };
        let (from, mut params) = match lead {
            // The page sorts the lead's rows rather than walking the sort's index for them;
            // `CROSS JOIN` fixes that order. The library condition still applies to every row.
            Some(Lead { ids, params, .. }) => {
                (format!("({ids}) lead CROSS JOIN {name} {alias} ON {alias}.id = lead.id"), params)
            }
            None => (format!("{name} {alias}"), Vec::new()),
        };
        let mut filter = format!("{alias}.library_id = ?");
        params.push(library.into());
        for Check { condition, params: more } in checks {
            filter += &format!(" AND {condition}");
            params.extend(more);
        }
        Ok(Source { select: self.select, from, joins: self.joins, filter, params, total })
    }
}
