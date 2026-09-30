//! Answering a query from an index: matching each word, scoring each document, and ranking the
//! sections.

use super::distance::distance;
use super::{Index, Kind, Section, words};

/// Leading articles, which a query may leave out and still match exactly.
const ARTICLES: [&str; 3] = ["the", "a", "an"];

/// Words longer than this are matched exactly or as a prefix only, which bounds the typo check.
const MAX_FUZZY_CHARS: usize = 32;

/// Query words beyond this are ignored.
const MAX_QUERY_WORDS: usize = 12;

/// How well one query word matched one document: lower is better, 0 is no match.
type Quality = u8;

fn quality(typos: u8, prefix: bool) -> Quality {
    1 + typos * 2 + u8::from(prefix)
}

/// How well a document matched the whole query: lower is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Score {
    inexact: bool,
    typos: u32,
    prefixes: u32,
    extra_words: u32,
}

impl Index {
    /// The best `limit` matches for `query` of each kind in `kinds` that has any, with the
    /// sections ordered by their best match, ties to the more specific kind.
    pub fn search(&self, query: &str, kinds: &[Kind], limit: usize) -> Vec<Section> {
        let mut query_words = words(query);
        query_words.truncate(MAX_QUERY_WORDS);
        if query_words.is_empty() || limit == 0 {
            return Vec::new();
        }

        // For each query word, how well every document matched it, and which ones did.
        let mut best: Vec<Vec<Quality>> = Vec::with_capacity(query_words.len());
        let mut matched: Vec<Vec<u32>> = Vec::with_capacity(query_words.len());
        for word in &query_words {
            let mut qualities = vec![0; self.documents.len()];
            let mut docs = Vec::new();
            for (w, q) in self.matching_words(word) {
                for &d in &self.postings[w as usize] {
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

        // The whole query's exact words, for the exact-name check.
        let exact: Option<Vec<u32>> = query_words
            .iter()
            .map(|word| self.vocabulary.binary_search(word).ok().map(|w| w as u32))
            .collect();

        // Candidates are the documents the most selective word matched; the others must match
        // every other word too.
        let fewest = (0..matched.len()).min_by_key(|&i| matched[i].len()).unwrap();
        let mut scored: [Vec<(Score, u32)>; 3] = Default::default();
        for &d in &matched[fewest] {
            let kind = self.documents[d as usize].kind;
            if !kinds.contains(&kind) {
                continue;
            }
            let Some(score) = self.score(d, &best, exact.as_deref()) else {
                continue;
            };
            scored[kind as usize].push((score, d));
        }

        let mut sections: Vec<(Score, Section)> = Vec::new();
        for (k, mut hits) in scored.into_iter().enumerate() {
            if hits.is_empty() {
                continue;
            }
            let total = hits.len();
            if hits.len() > limit {
                hits.select_nth_unstable(limit - 1);
                hits.truncate(limit);
            }
            // Document order is the tie-break order.
            hits.sort_unstable();
            sections.push((
                hits[0].0,
                Section {
                    kind: Kind::ALL[k],
                    ids: hits.iter().map(|&(_, d)| self.documents[d as usize].id).collect(),
                    total,
                },
            ));
        }
        sections.sort_by(|(a, x), (b, y)| a.cmp(b).then(x.kind.cmp(&y.kind)));
        sections.into_iter().map(|(_, section)| section).collect()
    }

    /// The document's score, if it matched every query word.
    fn score(&self, d: u32, best: &[Vec<Quality>], exact: Option<&[u32]>) -> Option<Score> {
        let mut typos = 0;
        let mut prefixes = 0;
        for qualities in best {
            let q = qualities[d as usize];
            if q == 0 {
                return None;
            }
            typos += u32::from((q - 1) / 2);
            prefixes += u32::from((q - 1) % 2);
        }
        let doc_words =
            &self.words[self.starts[d as usize] as usize..self.starts[d as usize + 1] as usize];
        let without_article = match doc_words.split_first() {
            Some((first, rest))
                if !rest.is_empty()
                    && ARTICLES.contains(&self.vocabulary[*first as usize].as_str()) =>
            {
                Some(rest)
            }
            _ => None,
        };
        let is_exact =
            exact.is_some_and(|exact| exact == doc_words || Some(exact) == without_article);
        Some(Score {
            inexact: !is_exact,
            typos,
            prefixes,
            extra_words: (doc_words.len() as u32).saturating_sub(best.len() as u32),
        })
    }

    /// The vocabulary words that `word` matches, and how well.
    fn matching_words(&self, word: &str) -> Vec<(u32, Quality)> {
        let chars: Vec<char> = word.chars().collect();
        let max_typos = match chars.len() {
            0..=3 => 0,
            4..=7 => 1,
            _ => 2,
        };
        let mut found = Vec::new();
        if max_typos == 0 || chars.len() > MAX_FUZZY_CHARS {
            // Exact and prefix matches are one contiguous run of the sorted vocabulary.
            for w in self.prefixed(word) {
                let prefix = self.vocabulary[w].len() != word.len();
                found.push((w as u32, quality(0, prefix)));
            }
            return found;
        }
        // Typos: every word starting with the same character.
        let first = &word[..chars[0].len_utf8()];
        let mut candidate: Vec<char> = Vec::with_capacity(MAX_FUZZY_CHARS);
        for w in self.prefixed(first) {
            candidate.clear();
            candidate.extend(self.vocabulary[w].chars().take(MAX_FUZZY_CHARS + max_typos as usize));
            if let Some((typos, prefix)) = distance(&chars, &candidate, max_typos) {
                found.push((w as u32, quality(typos, prefix)));
            }
        }
        found
    }

    /// The vocabulary indexes of the words that start with `prefix`.
    pub(super) fn prefixed(&self, prefix: &str) -> std::ops::Range<usize> {
        let start = self.vocabulary.partition_point(|w| w.as_str() < prefix);
        let end = start + self.vocabulary[start..].partition_point(|w| w.starts_with(prefix));
        start..end
    }
}
