//! Several indexes searched as one, such as a library's, which every user shares, and one user's
//! playlists', which are theirs alone.
//!
//! Each index ranks its own matches, and what they found is merged by rank: a match's rank is its
//! place in its section written out in values (`rank`), so ranks from two indexes compare as one
//! index's would. Sections that two indexes both have combine, keeping the best of both and
//! counting both; sections order by their best match, ties to the more specific kind, then by
//! group, as one index orders them. So the union finds exactly what one index of all their
//! documents would.

use std::collections::BTreeMap;

use super::query::RankedSection;
use super::{Index, Kind, Rank, Section, SectionPage};

/// Each section's matches from every index, with their ranks, and how many matched in all, by kind
/// and group.
type Merged = BTreeMap<(Kind, Option<String>), (Vec<(Rank, i64)>, usize)>;

/// Indexes searched as one.
#[derive(Debug, Clone, Copy)]
pub struct Union<'a>(pub &'a [&'a Index]);

impl Union<'_> {
    /// As [`Index::search_within`], over every index as one.
    pub fn search_within(
        &self,
        query: &str,
        kinds: &[Kind],
        limit: usize,
        keep: impl Fn(Kind, i64) -> bool,
    ) -> Vec<Section> {
        if let [index] = self.0 {
            return index.search_within(query, kinds, limit, keep);
        }
        if limit == 0 {
            return Vec::new();
        }
        let mut merged = Merged::new();
        for index in self.0 {
            for RankedSection { kind, group, hits, total } in
                index.ranked_sections(query, kinds, limit, &keep)
            {
                let (all, counted) = merged.entry((kind, group)).or_default();
                all.extend(hits);
                *counted += total;
            }
        }
        let mut sections: Vec<(Rank, Section)> = merged
            .into_iter()
            .map(|((kind, group), (mut hits, total))| {
                hits.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                hits.truncate(limit);
                let ids = hits.iter().map(|&(_, id)| id).collect();
                let best = hits.swap_remove(0).0;
                (best, Section { kind, group, ids, total })
            })
            .collect();
        sections.sort_by(|(a, x), (b, y)| {
            a.score.cmp(&b.score).then(x.kind.cmp(&y.kind)).then_with(|| x.group.cmp(&y.group))
        });
        sections.into_iter().map(|(_, section)| section).collect()
    }

    /// As [`Index::page`], over every index as one.
    pub fn page(
        &self,
        query: &str,
        kind: Kind,
        group: Option<&str>,
        after: Option<&Rank>,
        limit: usize,
        keep: impl Fn(Kind, i64) -> bool,
    ) -> SectionPage {
        if let [index] = self.0 {
            return index.page(query, kind, group, after, limit, keep);
        }
        let (mut hits, mut total) = (Vec::new(), 0);
        for index in self.0 {
            // One more than the page, so the merged page knows whether more follow it.
            let (found, counted) =
                index.ranked_page(query, kind, group, after, limit.saturating_add(1), &keep);
            hits.extend(found);
            total += counted;
        }
        hits.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        let more = hits.len() > limit;
        hits.truncate(limit);
        let ids = hits.iter().map(|&(_, id)| id).collect();
        let next = hits.pop().filter(|_| more).map(|(rank, _)| rank);
        SectionPage { ids, total, next }
    }

    /// As [`Index::matching`], over every index as one.
    pub fn matching(&self, query: &str, kind: Kind, keep: impl Fn(Kind, i64) -> bool) -> Vec<i64> {
        self.0.iter().flat_map(|index| index.matching(query, kind, &keep)).collect()
    }
}
