//! Answering a query from an index: matching each word, scoring each document, and ranking the
//! sections.

use super::distance::distance;
use super::{Index, Kind, Section, words};
use crate::fold::ARTICLES;

/// Words longer than this are matched exactly or as a prefix only, which bounds the typo check.
const MAX_FUZZY_CHARS: usize = 32;

/// Query words beyond this are ignored.
const MAX_QUERY_WORDS: usize = 12;

/// How well one query word matched one document: lower is better, 0 is no match.
type Quality = u8;

fn quality(typos: u8, prefix: bool) -> Quality {
    1 + typos * 2 + u8::from(prefix)
}

/// How many of `words` lead as an optional article: 1 when the first is an article and others
/// follow it, else 0.
fn leading_article<T>(words: &[T], is_article: impl Fn(&T) -> bool) -> usize {
    usize::from(words.len() > 1 && is_article(&words[0]))
}

/// How well a document matched the whole query: lower is better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Score {
    inexact: bool,
    typos: u32,
    /// The query's leading article went unmatched.
    skipped: bool,
    prefixes: u32,
    extra_words: u32,
}

impl Index {
    /// The best `limit` matches for `query` of each kind in `kinds` that has any, with the
    /// sections ordered by their best match, ties to the more specific kind. A leading article in
    /// a query of more than one word need not match.
    pub fn search(&self, query: &str, kinds: &[Kind], limit: usize) -> Vec<Section> {
        let mut query_words = words(query);
        query_words.truncate(MAX_QUERY_WORDS);
        if query_words.is_empty() || limit == 0 {
            return Vec::new();
        }
        let optional = leading_article(&query_words, |word| ARTICLES.contains(&word.as_str()));

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

        // The query's exact words after any leading article, for the exact-name check.
        let exact: Option<Vec<u32>> = query_words[optional..]
            .iter()
            .map(|word| self.vocabulary.binary_search(word).ok().map(|w| w as u32))
            .collect();

        // Candidates are the documents the most selective required word matched, so an optional
        // article never picks them; `score` drops those that miss another required word.
        let Some(fewest) = matched[optional..].iter().min_by_key(|docs| docs.len()) else {
            return Vec::new();
        };
        let mut scored: [Vec<(Score, u32)>; 3] = Default::default();
        for &d in fewest {
            let kind = self.documents[d as usize].kind;
            if !kinds.contains(&kind) {
                continue;
            }
            let Some(score) = self.score(d, &best, optional, exact.as_deref()) else {
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
        let doc_words =
            &self.words[self.starts[d as usize] as usize..self.starts[d as usize + 1] as usize];
        let article = leading_article(doc_words, |&w| {
            ARTICLES.contains(&self.vocabulary[w as usize].as_str())
        });
        Some(Score {
            inexact: exact != Some(&doc_words[article..]),
            typos,
            skipped,
            prefixes,
            extra_words: (doc_words.len() as u32).saturating_sub(matched),
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
