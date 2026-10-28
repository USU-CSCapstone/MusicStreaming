//! The `tag` and `tagMatch` filters: which of a list's rows carry the tags named
//! (`requirements/tags.md` §2, §7). Types are independent, so a list's rows match the tags of
//! every type named, each type's by `tagMatch`.

use std::collections::HashSet;

use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};
use serde::Deserialize;

use super::AnyId;
use super::page::{self, Check, Lead, Source, Table, Total};

/// The spec's `TagMatch`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagMatch {
    #[default]
    Any,
    All,
}

/// How a list's rows carry tags.
pub struct Tagged {
    /// The link table, such as `track_tags`.
    pub links: &'static str,
    /// Its column naming the row, such as `track_id`.
    pub owner: &'static str,
    /// The tags' stored count of such rows, such as `track_count`.
    pub count: &'static str,
}

/// How small a share of a list one type's tags must be for a page to start from their rows
/// rather than walk the sort's index and check each row: one row in this many at most. Starting
/// from a tag sorts all of its rows on every page; walking reads rows until a page's worth carry
/// it, which is quick for a common tag and reads the whole index for a rare one. Measured, the two
/// cross near one track in 80 at 100,000 tracks and one in 115 at 500,000, and a wrong choice near
/// there costs under a millisecond (`design/database.md` §4).
pub const LEAD_SHARE: i64 = 128;

/// The rows of `table` in `library` that start from `lead` and pass `check`, if given, and carry
/// the tags `filter` names, the way `tagged` links them. An artist's or album's
/// filter comes as `lead` or `check`; the tags plan around it ([`Tags::plan`]).
pub fn source(
    conn: &Connection,
    library: i64,
    table: &Table,
    tagged: &Tagged,
    lead: Option<Lead>,
    check: Option<Check>,
    filter: &TagFilter,
) -> rusqlite::Result<Source> {
    let tags = Tags::read(conn, library, tagged, filter)?;
    let rows = match tags {
        Tags::Groups(_) => page::library_number(conn, library, table.count)?,
        _ => 0,
    };
    let alone = lead.is_none() && check.is_none();
    let plan = tags.plan(tagged, table.alias, lead, rows, alone);
    let mut source =
        table.source(conn, library, plan.lead, Check::all(check.into_iter().chain(plan.check)))?;
    if let Some(total) = plan.total {
        source.total = total;
    }
    Ok(source)
}

/// A list's `tag` and `tagMatch` parameters.
#[derive(Debug, Clone, Default)]
pub struct TagFilter {
    /// The tags named, each as [`AnyId`] reads it.
    pub ids: Vec<AnyId>,
    pub tag_match: TagMatch,
}

impl TagFilter {
    /// The filter `tag`, comma-separated tag IDs, and `tag_match` make.
    pub fn new(tag: Option<&str>, tag_match: TagMatch) -> TagFilter {
        let ids = tag.into_iter().flat_map(|tag| tag.split(',')).filter(|id| !id.is_empty());
        TagFilter { ids: ids.map(AnyId::from).collect(), tag_match }
    }
}

/// The tags a filter names, as the library holds them.
pub enum Tags {
    /// The filter names no tags.
    Unfiltered,
    /// It names a tag the library does not have, so nothing matches.
    Nothing,
    /// Its tags of each type, each type's a group the rows must match.
    Groups(Vec<Group>),
}

/// The tags of one type a filter names, and how its rows must carry them.
pub struct Group {
    ids: Vec<i64>,
    tag_match: TagMatch,
    /// How many rows match the group at most, from the tags' stored counts. It is exact for one
    /// tag.
    size: i64,
}

/// What the filter makes of a list.
pub struct Plan {
    /// The rows a page starts from, the given lead or one group's.
    pub lead: Option<Lead>,
    /// The condition every row meets for the groups that do not lead.
    pub check: Option<Check>,
    /// The page's total, when the filter changes how it is known: `None` for no filter.
    pub total: Option<Total>,
}

impl Tags {
    /// The tags of `library` that `filter` names, grouped by type, with their stored counts of
    /// the rows `tagged` describes.
    pub fn read(
        conn: &Connection,
        library: i64,
        tagged: &Tagged,
        filter: &TagFilter,
    ) -> rusqlite::Result<Tags> {
        let TagFilter { ids, tag_match } = filter;
        let tag_match = *tag_match;
        if ids.is_empty() {
            return Ok(Tags::Unfiltered);
        }
        let Some(ids) = ids.iter().map(|id| id.0).collect::<Option<Vec<i64>>>() else {
            return Ok(Tags::Nothing);
        };
        let marks = vec!["?"; ids.len()].join(", ");
        let sql = format!(
            "SELECT id, type, {} FROM tags WHERE library_id = ? AND id IN ({marks}) ORDER BY type, id",
            tagged.count
        );
        let params = std::iter::once(library).chain(ids.iter().copied());
        let rows: Vec<(i64, String, i64)> = conn
            .prepare_cached(&sql)?
            .query_map(params_from_iter(params), |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut found: Vec<i64> = rows.iter().map(|&(id, _, _)| id).collect();
        let mut named = ids;
        found.sort_unstable();
        named.sort_unstable();
        named.dedup();
        if found != named {
            return Ok(Tags::Nothing);
        }
        let mut groups: Vec<(String, Group)> = Vec::new();
        for (id, tag_type, count) in rows {
            match groups.last_mut() {
                Some((last, group)) if *last == tag_type => {
                    group.ids.push(id);
                    group.size = match tag_match {
                        TagMatch::Any => group.size + count,
                        TagMatch::All => group.size.min(count),
                    };
                }
                _ => groups.push((tag_type, Group { ids: vec![id], tag_match, size: count })),
            }
        }
        Ok(Tags::Groups(groups.into_iter().map(|(_, group)| group).collect()))
    }

    /// How a list of `rows` rows of `tagged`, aliased `alias`, reads the filter. A list given a
    /// `lead` keeps it; otherwise its smallest group leads when it is a small enough share of the
    /// list ([`LEAD_SHARE`]). Every other group becomes a check. The total is the one tag's stored
    /// count where it is all that filters the list, which `alone` says, and is counted otherwise.
    pub fn plan(
        self,
        tagged: &Tagged,
        alias: &str,
        lead: Option<Lead>,
        rows: i64,
        alone: bool,
    ) -> Plan {
        self.plan_sharing(tagged, alias, lead, rows, alone, LEAD_SHARE)
    }

    /// [`Tags::plan`], with a group leading when it holds one row in `share` of the list at most:
    /// 0 has the smallest always lead, and `i64::MAX` has every group checked. For the benchmark
    /// that weighs one against the other.
    pub fn plan_sharing(
        self,
        tagged: &Tagged,
        alias: &str,
        lead: Option<Lead>,
        rows: i64,
        alone: bool,
        share: i64,
    ) -> Plan {
        let mut groups = match self {
            Tags::Unfiltered => return Plan { lead, check: None, total: None },
            Tags::Nothing => {
                let check = Check { condition: "0".to_owned(), params: Vec::new() };
                return Plan { lead, check: Some(check), total: Some(Total::Stored(0)) };
            }
            Tags::Groups(groups) => groups,
        };
        let total = match groups.as_slice() {
            [one] if alone && one.ids.len() == 1 => Total::Stored(one.size),
            _ => Total::Counted,
        };
        let lead = lead.or_else(|| {
            let smallest = (0..groups.len()).min_by_key(|&i| groups[i].size)?;
            (groups[smallest].size.saturating_mul(share) <= rows)
                .then(|| groups.swap_remove(smallest).lead(tagged))
        });
        let checks: Vec<Check> = groups.iter().map(|group| group.check(tagged, alias)).collect();
        Plan { lead, check: Check::all(checks), total: Some(total) }
    }
}

impl Tags {
    /// The IDs of the rows `tagged` describes that match every group, where the filter names
    /// tags: none for a tag the library does not have. Read from the smallest group's rows, each
    /// checked against the others, so it costs what that group holds.
    pub fn ids(self, conn: &Connection, tagged: &Tagged) -> rusqlite::Result<Option<HashSet<i64>>> {
        let mut groups = match self {
            Tags::Unfiltered => return Ok(None),
            Tags::Nothing => return Ok(Some(HashSet::new())),
            Tags::Groups(groups) => groups,
        };
        let Some(smallest) = (0..groups.len()).min_by_key(|&i| groups[i].size) else {
            return Ok(None);
        };
        let Lead { ids, mut params, .. } = groups.swap_remove(smallest).lead(tagged);
        let mut sql = format!("SELECT lead.id FROM ({ids}) lead");
        if let Some(check) = Check::all(groups.iter().map(|group| group.check(tagged, "lead"))) {
            sql += &format!(" WHERE {}", check.condition);
            params.extend(check.params);
        }
        let mut statement = conn.prepare_cached(&sql)?;
        let ids = statement.query_map(params_from_iter(params), |row| row.get(0))?;
        Ok(Some(ids.collect::<rusqlite::Result<_>>()?))
    }
}

impl Group {
    /// The group's rows, for a page to start from.
    fn lead(&self, tagged: &Tagged) -> Lead {
        let Tagged { links, owner, .. } = tagged;
        let ids = match (self.tag_match, self.ids.len()) {
            (_, 1) => format!("SELECT {owner} AS id FROM {links} WHERE tag_id = ?"),
            (TagMatch::Any, n) => format!(
                "SELECT DISTINCT {owner} AS id FROM {links} WHERE tag_id IN ({})",
                vec!["?"; n].join(", ")
            ),
            (TagMatch::All, n) => {
                vec![format!("SELECT {owner} AS id FROM {links} WHERE tag_id = ?"); n]
                    .join(" INTERSECT ")
            }
        };
        Lead { ids, params: self.params(), size: self.size }
    }

    /// The condition a row aliased `alias` meets to match the group: one lookup of its own links
    /// per tag at most, so a walk costs the same per row however many rows the tags have.
    fn check(&self, tagged: &Tagged, alias: &str) -> Check {
        let Tagged { links, owner, .. } = tagged;
        let condition = match self.tag_match {
            TagMatch::Any => format!(
                "EXISTS (SELECT 1 FROM {links} WHERE {owner} = {alias}.id AND tag_id IN ({}))",
                vec!["?"; self.ids.len()].join(", ")
            ),
            TagMatch::All => vec![
                format!(
                    "EXISTS (SELECT 1 FROM {links} WHERE {owner} = {alias}.id AND tag_id = ?)"
                );
                self.ids.len()
            ]
            .join(" AND "),
        };
        Check { condition, params: self.params() }
    }

    fn params(&self) -> Vec<Value> {
        self.ids.iter().map(|&id| Value::from(id)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAGGED: Tagged = Tagged { links: "track_tags", owner: "track_id", count: "track_count" };

    fn group(ids: &[i64], tag_match: TagMatch, size: i64) -> Group {
        Group { ids: ids.to_vec(), tag_match, size }
    }

    #[test]
    fn the_smallest_group_leads_when_it_is_a_small_share() {
        let tags = || {
            Tags::Groups(vec![
                group(&[1, 2], TagMatch::Any, 50_000),
                group(&[3], TagMatch::Any, 100),
            ])
        };
        let plan = tags().plan(&TAGGED, "t", None, 300_000, true);
        let lead = plan.lead.expect("the small group leads");
        assert_eq!(lead.ids, "SELECT track_id AS id FROM track_tags WHERE tag_id = ?");
        assert_eq!(lead.size, 100);
        assert!(plan.check.unwrap().condition.contains("tag_id IN (?, ?)"));
        assert!(matches!(plan.total, Some(Total::Counted)));

        // Beside fewer tracks, neither is small enough, and both are checked.
        let plan = tags().plan(&TAGGED, "t", None, 10_000, true);
        assert!(plan.lead.is_none());
        assert_eq!(plan.check.unwrap().params.len(), 3);
    }

    #[test]
    fn another_lead_is_kept() {
        let artist = Lead { ids: "SELECT 7 AS id".to_owned(), params: Vec::new(), size: 1 };
        let tags = Tags::Groups(vec![group(&[3], TagMatch::Any, 1)]);
        let plan = tags.plan(&TAGGED, "t", Some(artist), 300_000, false);
        assert_eq!(plan.lead.unwrap().ids, "SELECT 7 AS id");
        assert!(plan.check.is_some());
    }

    #[test]
    fn one_tag_alone_keeps_its_stored_count() {
        let one = || Tags::Groups(vec![group(&[3], TagMatch::All, 40)]);
        assert!(matches!(one().plan(&TAGGED, "t", None, 100, true).total, Some(Total::Stored(40))));
        assert!(matches!(one().plan(&TAGGED, "t", None, 100, false).total, Some(Total::Counted)));
        let nothing = Tags::Nothing.plan(&TAGGED, "t", None, 100, false);
        assert!(matches!(nothing.total, Some(Total::Stored(0))));
        assert_eq!(nothing.check.unwrap().condition, "0");
    }

    #[test]
    fn all_of_a_type_intersects_or_checks_each_tag() {
        let all = group(&[1, 2], TagMatch::All, 10);
        assert_eq!(
            all.lead(&TAGGED).ids,
            "SELECT track_id AS id FROM track_tags WHERE tag_id = ? INTERSECT \
             SELECT track_id AS id FROM track_tags WHERE tag_id = ?"
        );
        assert_eq!(all.check(&TAGGED, "t").condition.matches("EXISTS").count(), 2);
    }

    #[test]
    fn the_ids_matching_every_group_start_from_the_smallest() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE track_tags (track_id INTEGER, tag_id INTEGER);
             INSERT INTO track_tags VALUES (1, 10), (2, 10), (3, 10), (2, 11), (3, 12), (3, 20);",
        )
        .unwrap();
        let ids = |tags: Tags| {
            let mut ids: Vec<i64> =
                tags.ids(&conn, &TAGGED).unwrap().unwrap().into_iter().collect();
            ids.sort_unstable();
            ids
        };
        let genres = |tag_match| group(&[10, 11, 12], tag_match, 5);
        assert_eq!(ids(Tags::Groups(vec![genres(TagMatch::Any)])), [1, 2, 3]);
        assert_eq!(
            ids(Tags::Groups(vec![genres(TagMatch::Any), group(&[20], TagMatch::Any, 1)])),
            [3]
        );
        assert!(ids(Tags::Groups(vec![genres(TagMatch::All)])).is_empty());
        assert!(Tags::Unfiltered.ids(&conn, &TAGGED).unwrap().is_none());
        assert!(Tags::Nothing.ids(&conn, &TAGGED).unwrap().unwrap().is_empty());
    }

    #[test]
    fn lists_ids_separated_by_commas() {
        assert!(TagFilter::new(None, TagMatch::All).ids.is_empty());
        let ids = TagFilter::new(Some("1,,x"), TagMatch::Any).ids;
        assert_eq!(ids.iter().map(|id| id.0).collect::<Vec<_>>(), [Some(1), None]);
    }
}
