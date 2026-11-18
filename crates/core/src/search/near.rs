//! Near misses (`requirements/search.md` §8): when a search finds nothing, the names it nearly
//! found, since a user who mistyped badly still meant something.
//!
//! The query is matched again, more loosely than a search matches it, and only by a caller whose
//! search found nothing, so a search that finds something never pays for it:
//!
//! - **a wrong first letter**, so `baylor` nearly finds Taylor;
//! - **one more typo** than a search allows a word of its length, so `swfiit` nearly finds Swift;
//! - **one word left unmatched**, the one the name lacks, when the query has more than one beyond
//!   a leading article, so `taylor swift live` nearly finds Taylor Swift.
//!
//! A word of fewer than three letters is matched as a search matches it, or one letter would
//! nearly find every name there is. Only the near misses that leave the fewest words unmatched
//! are offered: where a name matches every word, one that matches only some of them would only
//! be noise beside it. They rank by fewer typos, then fewer words matched only as a prefix, then fewer words in the name beyond those
//! matched, then as a search breaks ties. Names that fold to the same words are one near miss.
//! What a search would leave out, the caller's `keep` leaves out here too, so a near miss never
//! names what the search could not have found.

use std::cmp::Reverse;
use std::collections::HashSet;

use super::distance::distance;
use super::query::{
    MAX_FUZZY_CHARS, MAX_QUERY_WORDS, Quality, leading_article, quality, typos_allowed,
};
use super::{Index, Kind, Union, words};
use crate::fold::ARTICLES;

/// Words shorter than this are matched as a search matches them, never more loosely.
const MIN_LOOSE_CHARS: usize = 3;

/// One near miss, with what places it among others: how nearly it matched, then the tie-break
/// of a search, written out in values, so near misses from two indexes compare as one index's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Miss {
    /// Words left unmatched, typos, words matched only as a prefix, and words beyond those
    /// matched: fewer is nearer.
    nearness: (u32, u32, u32, u32),
    kind: Kind,
    group: Option<String>,
    weight: Reverse<u32>,
    /// The name's folded words, which make two near misses one when they are the same.
    words: Vec<String>,
    id: i64,
}

impl Index {
    /// Up to `n` near misses for `query`, of `kinds` and kept by `keep`, nearest first, as each
    /// one's kind and ID: what a search that found nothing nearly found. No two have the same
    /// name, as folded.
    pub fn near_misses(
        &self,
        query: &str,
        kinds: &[Kind],
        n: usize,
        keep: impl Fn(Kind, i64) -> bool,
    ) -> Vec<(Kind, i64)> {
        self.ranked_misses(query, kinds, n, keep).into_iter().map(|m| (m.kind, m.id)).collect()
    }

    /// As [`Index::near_misses`], with what places each among others, to merge with another
    /// index's.
    pub(super) fn ranked_misses(
        &self,
        query: &str,
        kinds: &[Kind],
        n: usize,
        keep: impl Fn(Kind, i64) -> bool,
    ) -> Vec<Miss> {
        let mut query_words = words(query);
        query_words.truncate(MAX_QUERY_WORDS);
        if query_words.is_empty() || n == 0 {
            return Vec::new();
        }
        let optional = leading_article(&query_words, |word| ARTICLES.contains(&word.as_str()));
        let required = query_words.len() - optional;
        // One word may go unmatched where there are others to match.
        let may_miss = u32::from(required > 1);

        // For each query word, how well every document loosely matched it; and for each
        // document, how many of the required words it matched.
        let mut best: Vec<Vec<Quality>> = Vec::with_capacity(query_words.len());
        let mut matched = vec![0u32; self.ids.len()];
        let mut candidates = Vec::new();
        for (i, word) in query_words.iter().enumerate() {
            let mut qualities = vec![0; self.ids.len()];
            for (w, q) in self.loosely_matching_words(word) {
                for &d in self.posting(w as usize) {
                    let slot = &mut qualities[d as usize];
                    if *slot == 0 && i >= optional {
                        if matched[d as usize] == 0 {
                            candidates.push(d);
                        }
                        matched[d as usize] += 1;
                    }
                    if *slot == 0 || q < *slot {
                        *slot = q;
                    }
                }
            }
            best.push(qualities);
        }

        let mut scored: Vec<((u32, u32, u32, u32), u32)> = Vec::new();
        for d in candidates {
            let missed = required as u32 - matched[d as usize];
            let (kind, id) = (self.kinds[d as usize], self.ids[d as usize]);
            if missed > may_miss || !kinds.contains(&kind) || !keep(kind, id) {
                continue;
            }
            let (mut typos, mut prefixes, mut matched_all) = (0, 0, 0);
            for qualities in &best {
                let q = qualities[d as usize];
                if q > 0 {
                    typos += u32::from((q - 1) / 2);
                    prefixes += u32::from((q - 1) % 2);
                    matched_all += 1;
                }
            }
            let extra = (self.doc_words(d).count() as u32).saturating_sub(matched_all);
            scored.push(((missed, typos, prefixes, extra), d));
        }
        // Document order is a search's tie-break.
        scored.sort_unstable();
        let fewest_missed = scored.first().map(|((missed, ..), _)| *missed);
        scored.retain(|((missed, ..), _)| Some(*missed) == fewest_missed);

        let mut seen: HashSet<Vec<&str>> = HashSet::new();
        let mut misses = Vec::new();
        for (nearness, d) in scored {
            if misses.len() == n {
                break;
            }
            if !seen.insert(self.doc_words(d).collect()) {
                continue;
            }
            let at = d as usize;
            let (kind, group) = (self.kinds[at], self.groups_of[at]);
            let (weight, id) = (self.weights[at], self.ids[at]);
            let words = self.doc_words(d).map(str::to_owned).collect();
            let group = self.group(group);
            misses.push(Miss { nearness, kind, group, weight: Reverse(weight), words, id });
        }
        misses
    }

    /// The vocabulary words `word` loosely matches, and how well: whatever its first letter, with
    /// one typo more than a search allows a word as long, or as a search matches it if it is
    /// shorter than [`MIN_LOOSE_CHARS`].
    fn loosely_matching_words(&self, word: &str) -> Vec<(u32, Quality)> {
        let chars: Vec<char> = word.chars().collect();
        if chars.len() < MIN_LOOSE_CHARS || chars.len() > MAX_FUZZY_CHARS {
            return self.matching_words(word);
        }
        let max_typos = typos_allowed(chars.len()) + 1;
        let mut found = Vec::new();
        let mut candidate: Vec<char> = Vec::with_capacity(MAX_FUZZY_CHARS + max_typos as usize);
        for w in 0..self.vocabulary_len() {
            candidate.clear();
            candidate.extend(self.word(w).chars().take(MAX_FUZZY_CHARS + max_typos as usize));
            // Shorter by more than the typos allowed, it cannot be within them.
            if candidate.len() + usize::from(max_typos) < chars.len() {
                continue;
            }
            if let Some((typos, prefix)) = distance(&chars, &candidate, max_typos) {
                found.push((w as u32, quality(typos, prefix)));
            }
        }
        found
    }
}

impl Union<'_> {
    /// As [`Index::near_misses`], over every index as one.
    pub fn near_misses(
        &self,
        query: &str,
        kinds: &[Kind],
        n: usize,
        keep: impl Fn(Kind, i64) -> bool,
    ) -> Vec<(Kind, i64)> {
        // Each index's nearest `n` distinct names hold the union's: a name among the union's
        // nearest `n` has fewer than `n` names nearer than it anywhere, so in its own index too.
        let mut misses: Vec<Miss> =
            self.0.iter().flat_map(|index| index.ranked_misses(query, kinds, n, &keep)).collect();
        misses.sort_unstable();
        // Each index offers its own fewest missed; together, the fewest of all.
        let fewest_missed = misses.first().map(|miss| miss.nearness.0);
        misses.retain(|miss| Some(miss.nearness.0) == fewest_missed);
        let mut seen = HashSet::new();
        misses.retain(|miss| seen.insert(miss.words.clone()));
        misses.truncate(n);
        misses.into_iter().map(|miss| (miss.kind, miss.id)).collect()
    }
}
