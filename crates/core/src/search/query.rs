//! Answering a query from an index: matching each word, scoring each document, and ranking the
//! sections.

use std::cmp::Reverse;
use std::ops::Range;

use super::distance::distance;
use super::{Index, Kind, Rank, Section, SectionPage, words};
use crate::fold::ARTICLES;

/// Words longer than this are matched exactly or as a prefix only, which bounds the typo check.
pub(super) const MAX_FUZZY_CHARS: usize = 32;

/// Query words beyond this are ignored.
pub(super) const MAX_QUERY_WORDS: usize = 12;

/// How well one query word matched one document: lower is better, 0 is no match.
pub(super) type Quality = u8;

/// The quality of a match with `typos` typos, to a prefix of the word rather than all of it if
/// `prefix`. `Index::score` reads both back out of it.
pub(super) fn quality(typos: u8, prefix: bool) -> Quality {
    1 + typos * 2 + u8::from(prefix)
}

/// The typos a query word of `chars` characters may have and still match: none up to 3, one up to
/// 7, and two from 8.
pub(super) fn typos_allowed(chars: usize) -> u8 {
    match chars {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

/// How many of `words` lead as an optional article: 1 when the first is an article and others
/// follow it, else 0.
pub(super) fn leading_article<T>(words: &[T], is_article: impl Fn(&T) -> bool) -> usize {
    usize::from(words.len() > 1 && is_article(&words[0]))
}

/// One index's section, as [`Union`](super::Union) merges it: its best matches with their ranks and IDs, best
/// first, and how many matched in all.
pub(super) struct RankedSection {
    pub(super) kind: Kind,
    pub(super) group: Option<String>,
    pub(super) hits: Vec<(Rank, i64)>,
    pub(super) total: usize,
}

/// Where the matches of `kind`, and of the group with index `group`, go among `matches`' slots:
/// one for each kind, and one for each group of tags after them.
fn slot(kind: Kind, group: u32) -> usize {
    match kind {
        Kind::Tag => Kind::Tag as usize + group as usize,
        kind => kind as usize,
    }
}

/// Keeps the best `n` of `hits`, best first. Document order is the tie-break order, so the
/// order is total, and the same `hits` always rank the same.
fn best(hits: &mut Vec<(Score, u32)>, n: usize) {
    if n == 0 {
        hits.clear();
        return;
    }
    if hits.len() > n {
        hits.select_nth_unstable(n - 1);
        hits.truncate(n);
    }
    hits.sort_unstable();
}

/// How well a document matched the whole query: lower is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Score {
    pub inexact: bool,
    pub typos: u32,
    /// The query's leading article went unmatched.
    pub skipped: bool,
    pub prefixes: u32,
    pub extra_words: u32,
}

impl Index {
    /// The best `limit` matches for `query` of each kind in `kinds` that has any, a section for
    /// each group of a kind that has groups, with the sections ordered by their best match, ties
    /// to the more specific kind, then by group. A leading article in a query of more than one
    /// word need not match.
    pub fn search(&self, query: &str, kinds: &[Kind], limit: usize) -> Vec<Section> {
        self.search_within(query, kinds, limit, |_, _| true)
    }

    /// As [`Index::search`], over only the documents `keep` keeps by kind and ID, so a filter
    /// narrows the matches before they are ranked and counted, wherever search runs
    /// (`requirements/search.md` §5).
    pub fn search_within(
        &self,
        query: &str,
        kinds: &[Kind],
        limit: usize,
        keep: impl Fn(Kind, i64) -> bool,
    ) -> Vec<Section> {
        if limit == 0 {
            return Vec::new();
        }
        let mut sections: Vec<(Score, u32, Section)> = Vec::new();
        for mut hits in self.matches(query, kinds, keep) {
            let Some(&(_, d)) = hits.first() else {
                continue;
            };
            let (kind, group) = (self.kinds[d as usize], self.groups_of[d as usize]);
            let total = hits.len();
            best(&mut hits, limit);
            let ids = self.hit_ids(&hits);
            let section = Section { kind, group: self.group(group), ids, total };
            sections.push((hits[0].0, group, section));
        }
        // Groups compare as their indexes do, since they are sorted.
        sections.sort_by(|(a, g, x), (b, h, y)| a.cmp(b).then(x.kind.cmp(&y.kind)).then(g.cmp(h)));
        sections.into_iter().map(|(_, _, section)| section).collect()
    }

    /// The best `limit` matches for `query` of `kind`, and of `group` for a kind that has groups,
    /// that `keep` keeps and that rank after `after`, ranked as [`Index::search_within`] ranks
    /// them, so a section can be read a page at a time. `total` counts every match, and `next` is
    /// the last one's rank while more follow it. A rank from an index since rebuilt continues
    /// where it left off. A group the index lacks has no matches.
    pub fn page(
        &self,
        query: &str,
        kind: Kind,
        group: Option<&str>,
        after: Option<&Rank>,
        limit: usize,
        keep: impl Fn(Kind, i64) -> bool,
    ) -> SectionPage {
        let Some(slot) = self.slot_of(kind, group) else {
            return SectionPage { ids: Vec::new(), total: 0, next: None };
        };
        let mut hits = std::mem::take(&mut self.matches(query, &[kind], keep)[slot]);
        let total = hits.len();
        if let Some(after) = after {
            hits.retain(|&(score, d)| self.ranks_after(score, d, after));
        }
        let more = hits.len() > limit;
        best(&mut hits, limit);
        let next = hits.last().filter(|_| more).map(|&(score, d)| self.rank(score, d));
        SectionPage { ids: self.hit_ids(&hits), total, next }
    }

    /// Each section's best `limit` matches for `query` of each of `kinds` that `keep` keeps,
    /// with their ranks, best first, and how many matched in all: what [`Union`](super::Union) merges with
    /// another index's.
    pub(super) fn ranked_sections(
        &self,
        query: &str,
        kinds: &[Kind],
        limit: usize,
        keep: impl Fn(Kind, i64) -> bool,
    ) -> Vec<RankedSection> {
        let mut sections = Vec::new();
        for mut hits in self.matches(query, kinds, keep) {
            let Some(&(_, d)) = hits.first() else {
                continue;
            };
            let (kind, group) = (self.kinds[d as usize], self.groups_of[d as usize]);
            let total = hits.len();
            best(&mut hits, limit);
            let hits = hits.iter().map(|&(score, d)| (self.rank(score, d), self.ids[d as usize]));
            sections.push(RankedSection {
                kind,
                group: self.group(group),
                hits: hits.collect(),
                total,
            });
        }
        sections
    }

    /// As [`Index::page`], with each match's rank, and up to `limit` of them: what [`Union`]
    /// merges with another index's page.
    pub(super) fn ranked_page(
        &self,
        query: &str,
        kind: Kind,
        group: Option<&str>,
        after: Option<&Rank>,
        limit: usize,
        keep: impl Fn(Kind, i64) -> bool,
    ) -> (Vec<(Rank, i64)>, usize) {
        let Some(slot) = self.slot_of(kind, group) else {
            return (Vec::new(), 0);
        };
        let mut hits = std::mem::take(&mut self.matches(query, &[kind], keep)[slot]);
        let total = hits.len();
        if let Some(after) = after {
            hits.retain(|&(score, d)| self.ranks_after(score, d, after));
        }
        best(&mut hits, limit);
        let hits = hits.iter().map(|&(score, d)| (self.rank(score, d), self.ids[d as usize]));
        (hits.collect(), total)
    }

    /// The slot of `kind`'s section, or `group`'s of a kind that has groups, among `matches`',
    /// if the index has such a group.
    fn slot_of(&self, kind: Kind, group: Option<&str>) -> Option<usize> {
        let group = match group {
            None => 0,
            Some(group) => match self.groups.binary_search_by(|g| g.as_str().cmp(group)) {
                Ok(found) if found > 0 => found as u32,
                _ => return None,
            },
        };
        Some(slot(kind, group))
    }

    /// The IDs of every match for `query` of `kind` that `keep` keeps, in no order, for a caller
    /// that orders them itself, such as by a list's sort. Leaving out the ranking saves sorting
    /// them all.
    pub fn matching(&self, query: &str, kind: Kind, keep: impl Fn(Kind, i64) -> bool) -> Vec<i64> {
        self.matches(query, &[kind], keep).iter().flat_map(|hits| self.hit_ids(hits)).collect()
    }

    /// The group with index `group`, if it is one: 0 is none.
    pub(super) fn group(&self, group: u32) -> Option<String> {
        (group > 0).then(|| self.groups[group as usize].clone())
    }

    /// The IDs of the documents of `hits`, in their order.
    fn hit_ids(&self, hits: &[(Score, u32)]) -> Vec<i64> {
        hits.iter().map(|&(_, d)| self.ids[d as usize]).collect()
    }

    /// The words of document `d`, folded, in name order.
    pub(super) fn doc_words(&self, d: u32) -> impl Iterator<Item = &str> {
        self.document_words(d as usize).iter().map(|&w| self.word(w as usize))
    }

    /// The rank of document `d`, scored `score`.
    fn rank(&self, score: Score, d: u32) -> Rank {
        let words = self.doc_words(d).map(str::to_owned).collect();
        Rank { score, weight: Reverse(self.weights[d as usize]), words, id: self.ids[d as usize] }
    }

    /// Whether document `d`, scored `score`, ranks after `after`. Its words are read only when
    /// the scores and weights tie.
    fn ranks_after(&self, score: Score, d: u32, after: &Rank) -> bool {
        score
            .cmp(&after.score)
            .then_with(|| Reverse(self.weights[d as usize]).cmp(&after.weight))
            .then_with(|| self.doc_words(d).cmp(after.words.iter().map(String::as_str)))
            .then_with(|| self.ids[d as usize].cmp(&after.id))
            .is_gt()
    }

    /// Every document of each of `kinds` that matches `query` and that `keep` keeps, with its
    /// score, in a slot for each section ([`slot`]). A leading article in a query of more than
    /// one word need not match.
    fn matches(
        &self,
        query: &str,
        kinds: &[Kind],
        keep: impl Fn(Kind, i64) -> bool,
    ) -> Vec<Vec<(Score, u32)>> {
        // An empty index has no groups at all, not even the empty one.
        let mut scored: Vec<Vec<(Score, u32)>> =
            vec![Vec::new(); slot(Kind::Tag, 0) + self.groups.len().max(1)];
        let mut query_words = words(query);
        query_words.truncate(MAX_QUERY_WORDS);
        if query_words.is_empty() {
            return scored;
        }
        let optional = leading_article(&query_words, |word| ARTICLES.contains(&word.as_str()));

        // For each query word, how well every document matched it, and which ones did.
        let mut best: Vec<Vec<Quality>> = Vec::with_capacity(query_words.len());
        let mut matched: Vec<Vec<u32>> = Vec::with_capacity(query_words.len());
        for word in &query_words {
            let mut qualities = vec![0; self.ids.len()];
            let mut docs = Vec::new();
            for (w, q) in self.matching_words(word) {
                for &d in self.posting(w as usize) {
                    let slot = &mut qualities[d as usize];
                    if *slot == 0 {
                        docs.push(d);
                    }
                    if *slot == 0 || q < *slot {
                        *slot = q;
                    }
                }
            }
            best.push(qualities);
            matched.push(docs);
        }

        // The query's exact words after any leading article, for the exact-name check.
        let exact: Option<Vec<u32>> =
            query_words[optional..].iter().map(|word| self.find(word).map(|w| w as u32)).collect();

        // Candidates are the documents the most selective required word matched, so an optional
        // article never picks them; `score` drops those that miss another required word.
        let Some(fewest) = matched[optional..].iter().min_by_key(|docs| docs.len()) else {
            return scored;
        };
        for &d in fewest {
            let kind = self.kinds[d as usize];
            if !kinds.contains(&kind) || !keep(kind, self.ids[d as usize]) {
                continue;
            }
            let Some(score) = self.score(d, &best, optional, exact.as_deref()) else {
                continue;
            };
            scored[slot(kind, self.groups_of[d as usize])].push((score, d));
        }
        scored
    }

    /// The document's score, if it matched every query word but the first `optional`. `exact` is
    /// the query's words after any leading article, `None` if one is not in the vocabulary; the
    /// document's words after its own leading article must equal them for an exact match.
    fn score(
        &self,
        d: u32,
        best: &[Vec<Quality>],
        optional: usize,
        exact: Option<&[u32]>,
    ) -> Option<Score> {
        let mut typos = 0;
        let mut prefixes = 0;
        let mut skipped = false;
        for (i, qualities) in best.iter().enumerate() {
            match qualities[d as usize] {
                0 if i < optional => skipped = true,
                0 => return None,
                q => {
                    typos += u32::from((q - 1) / 2);
                    prefixes += u32::from((q - 1) % 2);
                }
            }
        }
        let matched = (best.len() - usize::from(skipped)) as u32;
        let doc_words = self.document_words(d as usize);
        let article = leading_article(doc_words, |&w| ARTICLES.contains(&self.word(w as usize)));
        Some(Score {
            inexact: exact != Some(&doc_words[article..]),
            typos,
            skipped,
            prefixes,
            extra_words: (doc_words.len() as u32).saturating_sub(matched),
        })
    }

    /// The vocabulary words that `word` matches, and how well.
    pub(super) fn matching_words(&self, word: &str) -> Vec<(u32, Quality)> {
        let chars: Vec<char> = word.chars().collect();
        let max_typos = typos_allowed(chars.len());
        let mut found = Vec::new();
        if max_typos == 0 || chars.len() > MAX_FUZZY_CHARS {
            // Exact and prefix matches are one contiguous run of the sorted vocabulary.
            for w in self.prefixed(word) {
                let prefix = self.word(w).len() != word.len();
                found.push((w as u32, quality(0, prefix)));
            }
            return found;
        }
        // Typos: every word starting with the same character.
        let first = &word[..chars[0].len_utf8()];
        let mut candidate: Vec<char> = Vec::with_capacity(MAX_FUZZY_CHARS);
        for w in self.prefixed(first) {
            candidate.clear();
            candidate.extend(self.word(w).chars().take(MAX_FUZZY_CHARS + max_typos as usize));
            if let Some((typos, prefix)) = distance(&chars, &candidate, max_typos) {
                found.push((w as u32, quality(typos, prefix)));
            }
        }
        found
    }

    /// The vocabulary indexes of the words that start with `prefix`.
    pub(super) fn prefixed(&self, prefix: &str) -> Range<usize> {
        let start = self.partition(0..self.vocabulary_len(), |w| w < prefix);
        let end = self.partition(start..self.vocabulary_len(), |w| w.starts_with(prefix));
        start..end
    }

    /// The vocabulary index of `word`, if it is there.
    fn find(&self, word: &str) -> Option<usize> {
        let w = self.partition(0..self.vocabulary_len(), |v| v < word);
        (w < self.vocabulary_len() && self.word(w) == word).then_some(w)
    }

    /// The first vocabulary index in `within` whose word `before` is false of, where it holds of
    /// every word up to some point and of none after, as `slice::partition_point` finds it.
    fn partition(&self, mut within: Range<usize>, before: impl Fn(&str) -> bool) -> usize {
        while within.start < within.end {
            let middle = within.start + (within.end - within.start) / 2;
            if before(self.word(middle)) {
                within.start = middle + 1;
            } else {
                within.end = middle;
            }
        }
        within.start
    }
}
