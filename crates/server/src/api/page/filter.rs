//! Filtered lists: the rows a page starts from, and where its total comes from
//! (`design/database.md` §4).

use rusqlite::Connection;
use rusqlite::types::Value;

use super::{Source, Total, library_number};

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

impl Check {
    /// One check that a row passes when it passes every one of `checks`; `None` for none.
    pub fn all(checks: impl IntoIterator<Item = Check>) -> Option<Check> {
        checks.into_iter().reduce(|all, check| Check {
            condition: format!("({}) AND ({})", all.condition, check.condition),
            params: [all.params, check.params].concat(),
        })
    }
}

impl Table {
    /// The rows of the table in `library`, or those of `lead`, that meet `check` if there is
    /// one. The total is the library's stored count, or the lead's size, and is counted only
    /// where there is a check. `library` comes from the caller's `Scope`, so the caller reaches
    /// it.
    pub fn source(
        &self,
        conn: &Connection,
        library: i64,
        lead: Option<Lead>,
        check: Option<Check>,
    ) -> rusqlite::Result<Source> {
        let (name, alias) = (self.name, self.alias);
        let total = match (&lead, &check) {
            (None, None) => Total::Stored(library_number(conn, library, self.count)?),
            (Some(lead), None) => Total::Stored(lead.size),
            (_, Some(_)) => Total::Counted,
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
        if let Some(Check { condition, params: more }) = check {
            filter += &format!(" AND {condition}");
            params.extend(more);
        }
        Ok(Source { select: self.select, from, joins: self.joins, filter, params, total })
    }
}
