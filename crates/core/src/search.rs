//! Search matching and ranking (`requirements/search.md` §3–§4, `design/general.md` §3).
//!
//! One implementation for the server and, later, the device, so both return the same results
//! in the same order. Pure and deterministic: nothing here depends on the platform, the locale,
//! or hash order.
//!
//! Names are folded into words: accents dropped, case folded, apostrophes joining (so `aint`
//! is `Ain't`), anything else that is not a letter or digit separating. Every word of the query
//! must match a different-or-same word of the name, in any order, in one of three ways:
//!
//! - **exactly**;
//! - **as a prefix**, so results appear from the first character typed;
//! - **with typos**: one for a word of 4 to 7 characters, two from 8, counting a transposed
//!   pair as one. The first character must be right, which keeps the words to try few enough
//!   to meet the time budget, and is the letter people mistype least.
//!
//! Matches rank by, in order: the whole name matching exactly, leading article optional (the
//! guardrail that keeps an exact match from being buried); fewer typos; fewer words matched only
//! as a prefix; fewer words in the name beyond the query's. Ties break by kind (tracks, then
//! albums, then artists), then folded name, then ID.

use std::collections::{HashMap, HashSet};

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// What a name names. The order is the tie-break: the more specific thing first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Track,
    Album,
    Artist,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Track, Kind::Album, Kind::Artist];
}

/// A thing to find by its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub kind: Kind,
    pub id: i64,
    pub name: String,
}

/// One kind's matches, best first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub kind: Kind,
    /// The best `limit` matches' IDs.
    pub ids: Vec<i64>,
    /// How many matched in all.
    pub total: usize,
}

/// Leading articles, which a query may leave out and still match exactly.
const ARTICLES: [&str; 3] = ["the", "a", "an"];

/// Words longer than this are matched exactly or as a prefix only, which bounds the typo check.
const MAX_FUZZY_CHARS: usize = 32;

/// Query words beyond this are ignored.
const MAX_QUERY_WORDS: usize = 12;

/// The folded words of `text`.
pub fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for ch in text.nfd() {
        if is_combining_mark(ch) || ch == '\'' || ch == '\u{2019}' {
            continue;
        }
        if ch.is_alphanumeric() {
            match ch {
                // Letters with no decomposition that people type without their mark.
                'ß' => word.push_str("ss"),
                'æ' | 'Æ' => word.push_str("ae"),
                'œ' | 'Œ' => word.push_str("oe"),
                'ø' | 'Ø' => word.push('o'),
                'ł' | 'Ł' => word.push('l'),
                'đ' | 'Đ' => word.push('d'),
                'þ' | 'Þ' => word.push_str("th"),
                _ => word.extend(ch.to_lowercase()),
            }
        } else if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Names by their words, ready to search.
#[derive(Debug, Default)]
pub struct Index {
    /// Every distinct word, sorted.
    vocabulary: Vec<String>,
    /// For each word, the documents holding it, ascending.
    postings: Vec<Vec<u32>>,
    /// Documents in tie-break order: by kind, then folded name, then ID.
    documents: Vec<Entry>,
    /// Each document's words as vocabulary indexes, in name order: `words[starts[d]..starts[d + 1]]`.
    words: Vec<u32>,
    starts: Vec<u32>,
}

#[derive(Debug)]
struct Entry {
    kind: Kind,
    id: i64,
}

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
    pub fn build(documents: impl IntoIterator<Item = Document>) -> Index {
        let folded: Vec<(Kind, Vec<String>, i64)> = documents
            .into_iter()
            .map(|doc| (doc.kind, words(&doc.name), doc.id))
            .collect();

        // Sorted, so vocabulary indexes compare as the words do. Set order never shows.
        let distinct: HashSet<&str> = folded
            .iter()
            .flat_map(|(_, words, _)| words.iter().map(String::as_str))
            .collect();
        let mut vocabulary: Vec<&str> = distinct.into_iter().collect();
        vocabulary.sort_unstable();
        let position: HashMap<&str, u32> = vocabulary
            .iter()
            .enumerate()
            .map(|(i, word)| (*word, i as u32))
            .collect();

        // In tie-break order. Comparing indexes is comparing the words, and cheaper.
        let mut sorted: Vec<(Kind, Vec<u32>, i64)> = folded
            .iter()
            .map(|(kind, words, id)| {
                (
                    *kind,
                    words.iter().map(|w| position[w.as_str()]).collect(),
                    *id,
                )
            })
            .collect();
        sorted.sort_unstable();

        let mut index = Index {
            vocabulary: vocabulary.iter().map(|word| (*word).to_owned()).collect(),
            postings: vec![Vec::new(); vocabulary.len()],
            documents: Vec::with_capacity(sorted.len()),
            starts: Vec::with_capacity(sorted.len() + 1),
            ..Index::default()
        };
        index.starts.push(0);
        for (d, (kind, doc_words, id)) in sorted.into_iter().enumerate() {
            for &w in &doc_words {
                let posting = &mut index.postings[w as usize];
                if posting.last() != Some(&(d as u32)) {
                    posting.push(d as u32);
                }
            }
            index.words.extend(doc_words);
            index.starts.push(index.words.len() as u32);
            index.documents.push(Entry { kind, id });
        }
        index
    }

    pub fn len(&self) -> usize {
        self.documents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

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
        let fewest = (0..matched.len())
            .min_by_key(|&i| matched[i].len())
            .unwrap();
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
                    ids: hits
                        .iter()
                        .map(|&(_, d)| self.documents[d as usize].id)
                        .collect(),
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
            candidate.extend(
                self.vocabulary[w]
                    .chars()
                    .take(MAX_FUZZY_CHARS + max_typos as usize),
            );
            if let Some((typos, prefix)) = distance(&chars, &candidate, max_typos) {
                found.push((w as u32, quality(typos, prefix)));
            }
        }
        found
    }

    /// The vocabulary indexes of the words that start with `prefix`.
    fn prefixed(&self, prefix: &str) -> std::ops::Range<usize> {
        let start = self.vocabulary.partition_point(|w| w.as_str() < prefix);
        let end = start + self.vocabulary[start..].partition_point(|w| w.starts_with(prefix));
        start..end
    }
}

/// The typos between `query` and `word`, counting a transposed pair as one, and whether that
/// is to a prefix of `word` rather than all of it. `None` if more than `max`.
fn distance(query: &[char], word: &[char], max: u8) -> Option<(u8, bool)> {
    let m = query.len();
    let max = usize::from(max);
    // One column per character of `word`: column j holds the distance from each prefix of
    // `query` to `word[..j]`. The two before it are kept, for transpositions.
    let mut before: Vec<usize> = vec![0; m + 1];
    let mut previous: Vec<usize> = (0..=m).collect();
    let mut current: Vec<usize> = vec![0; m + 1];
    let mut to_prefix = m;
    let mut to_whole = if word.is_empty() { m } else { usize::MAX };
    for j in 1..=word.len() {
        current[0] = j;
        for i in 1..=m {
            let cost = usize::from(query[i - 1] != word[j - 1]);
            let mut d = (previous[i] + 1)
                .min(current[i - 1] + 1)
                .min(previous[i - 1] + cost);
            if i > 1 && j > 1 && query[i - 1] == word[j - 2] && query[i - 2] == word[j - 1] {
                d = d.min(before[i - 2] + 1);
            }
            current[i] = d;
        }
        to_prefix = to_prefix.min(current[m]);
        if j == word.len() {
            to_whole = current[m];
        }
        // A column's distances never shrink by more than a transposition can undo, so once
        // two columns in a row are past `max`, nothing further can come back within it.
        let past = |column: &[usize]| column.iter().all(|&d| d > max);
        if past(&current) && past(&previous) {
            break;
        }
        std::mem::swap(&mut before, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    if to_whole <= max && to_whole <= to_prefix {
        Some((to_whole as u8, false))
    } else if to_prefix <= max {
        Some((to_prefix as u8, true))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(kind: Kind, id: i64, name: &str) -> Document {
        Document {
            kind,
            id,
            name: name.to_owned(),
        }
    }

    fn library() -> Index {
        Index::build([
            doc(Kind::Artist, 1, "Taylor Swift"),
            doc(Kind::Artist, 2, "Björk"),
            doc(Kind::Artist, 3, "The Beatles"),
            doc(Kind::Artist, 4, "Weezer"),
            doc(Kind::Album, 10, "Weezer (Blue Album)"),
            doc(Kind::Album, 11, "Blue"),
            doc(Kind::Track, 20, "Say It Ain't So"),
            doc(Kind::Track, 21, "Blue"),
            doc(Kind::Track, 22, "Beatles Medley"),
            doc(Kind::Track, 23, "Hello"),
            doc(Kind::Track, 24, "Hello World"),
            doc(Kind::Track, 25, "Hallo"),
            doc(Kind::Track, 26, "Swiftly"),
        ])
    }

    /// Each section's kind and IDs, in order.
    fn find(index: &Index, query: &str) -> Vec<(Kind, Vec<i64>)> {
        index
            .search(query, &Kind::ALL, 10)
            .into_iter()
            .map(|section| (section.kind, section.ids))
            .collect()
    }

    /// The single best match.
    fn top(index: &Index, query: &str) -> Option<i64> {
        index
            .search(query, &Kind::ALL, 1)
            .first()
            .map(|section| section.ids[0])
    }

    #[test]
    fn folds_accents_case_and_apostrophes() {
        assert_eq!(words("Björk"), ["bjork"]);
        assert_eq!(words("SAY it Ain’t so!"), ["say", "it", "aint", "so"]);
        assert_eq!(words("AC/DC — Live"), ["ac", "dc", "live"]);
        assert_eq!(words("Straße Øresund Ærø"), ["strasse", "oresund", "aero"]);
        assert_eq!(words("  ...  "), Vec::<String>::new());
    }

    #[test]
    fn typos_do_not_get_in_the_way() {
        let index = library();
        assert_eq!(
            top(&index, "tylor sweft"),
            Some(1),
            "missing and wrong letters"
        );
        assert_eq!(top(&index, "taylro swift"), Some(1), "transposed letters");
        assert_eq!(top(&index, "taylorr swift"), Some(1), "doubled letters");
    }

    #[test]
    fn accents_case_and_word_order_are_irrelevant() {
        let index = library();
        assert_eq!(top(&index, "bjork"), Some(2));
        assert_eq!(top(&index, "BJÖRK"), Some(2));
        assert_eq!(top(&index, "swift taylor"), Some(1));
    }

    #[test]
    fn partial_input_matches() {
        let index = library();
        assert_eq!(top(&index, "tay"), Some(1));
        assert_eq!(top(&index, "say it ain"), Some(20));
        assert_eq!(top(&index, "say it aint so"), Some(20));
    }

    #[test]
    fn leading_articles_do_not_matter() {
        let index = library();
        // "beatles" is The Beatles exactly, above the track that merely contains the word.
        assert_eq!(
            find(&index, "beatles"),
            [(Kind::Artist, vec![3]), (Kind::Track, vec![22])]
        );
        assert_eq!(top(&index, "the beatles"), Some(3));
    }

    #[test]
    fn exact_matches_always_win() {
        let index = library();
        // Exact, then the same word with more after it, then one typo away.
        assert_eq!(find(&index, "hello"), [(Kind::Track, vec![23, 24, 25])]);
        assert_eq!(find(&index, "hallo"), [(Kind::Track, vec![25, 23, 24])]);
        // A prefix never beats the whole word.
        assert_eq!(
            find(&index, "swift"),
            [(Kind::Artist, vec![1]), (Kind::Track, vec![26])]
        );
    }

    #[test]
    fn sections_order_by_their_best_match() {
        let index = library();
        // The artist is exact; the album only contains the word.
        assert_eq!(
            find(&index, "weezer"),
            [(Kind::Artist, vec![4]), (Kind::Album, vec![10])]
        );
        // Equally exact: the track, then the album.
        assert_eq!(
            find(&index, "blue"),
            [(Kind::Track, vec![21]), (Kind::Album, vec![11, 10])]
        );
    }

    #[test]
    fn totals_count_past_the_limit() {
        let index = library();
        let sections = index.search("h", &Kind::ALL, 1);
        assert_eq!(sections.len(), 1);
        // "Hallo" and "Hello" tie, and "hallo" sorts first.
        assert_eq!(sections[0].ids, [25]);
        assert_eq!(sections[0].total, 3);
    }

    #[test]
    fn kinds_filter_the_sections() {
        let index = library();
        assert_eq!(
            index.search("blue", &[Kind::Album], 10),
            [Section {
                kind: Kind::Album,
                ids: vec![11, 10],
                total: 2
            }]
        );
    }

    #[test]
    fn nothing_to_match_finds_nothing() {
        let index = library();
        assert!(index.search("", &Kind::ALL, 10).is_empty());
        assert!(index.search("?!", &Kind::ALL, 10).is_empty());
        assert!(index.search("zzzz", &Kind::ALL, 10).is_empty());
        assert!(Index::build([]).search("blue", &Kind::ALL, 10).is_empty());
    }

    #[test]
    fn the_first_letter_must_be_right() {
        let index = library();
        assert_eq!(top(&index, "baylor"), None);
    }

    #[test]
    fn short_words_allow_no_typos() {
        let index = Index::build([doc(Kind::Track, 1, "Cat"), doc(Kind::Track, 2, "Bat")]);
        assert_eq!(find(&index, "cat"), [(Kind::Track, vec![1])]);
    }

    #[test]
    fn distance_counts_typos_to_the_word_or_a_prefix() {
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        let d = |a: &str, b: &str, max| distance(&chars(a), &chars(b), max);
        assert_eq!(d("swift", "swift", 1), Some((0, false)));
        assert_eq!(d("swi", "swift", 1), Some((0, true)));
        assert_eq!(d("sweft", "swift", 1), Some((1, false)));
        assert_eq!(
            d("tyalor", "taylor", 1),
            Some((1, false)),
            "a transposition is one typo"
        );
        assert_eq!(d("swfi", "swiftly", 1), Some((1, true)));
        assert_eq!(d("swift", "sw", 1), None);
        assert_eq!(d("abcdef", "uvwxyz", 2), None);
    }

    #[test]
    fn ties_break_by_name_then_id() {
        let index = Index::build([
            doc(Kind::Track, 3, "Love B"),
            doc(Kind::Track, 2, "Love A"),
            doc(Kind::Track, 1, "Love A"),
        ]);
        assert_eq!(find(&index, "love"), [(Kind::Track, vec![1, 2, 3])]);
    }

    /// Build and query times at the full-scale target: 500,000 tracks, 50,000 albums, and
    /// 20,000 artists named from a Zipf-distributed vocabulary. Run with
    /// `cargo test --release -p jewelcase-core search::tests::full_scale -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn full_scale() {
        // A deterministic generator, so every run measures the same library.
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let letters: Vec<char> = "abcdefghijklmnoprstuvwy".chars().collect();
        let vocabulary: Vec<String> = (0..40_000)
            .map(|_| {
                let len = 2 + (next() % 8) as usize;
                (0..len)
                    .map(|_| letters[(next() % letters.len() as u64) as usize])
                    .collect()
            })
            .collect();
        let mut word = || {
            // Zipf-like: small indexes are far more common.
            let r = (next() % 1_000_000) as f64 / 1_000_000.0;
            let i = ((vocabulary.len() as f64).powf(r) - 1.0) as usize;
            vocabulary[i.min(vocabulary.len() - 1)].clone()
        };
        let mut documents = Vec::new();
        for (kind, count, words) in [
            (Kind::Track, 500_000, 4),
            (Kind::Album, 50_000, 3),
            (Kind::Artist, 20_000, 2),
        ] {
            for id in 0..count {
                let n = 1 + (id % words) as usize;
                let name = (0..n).map(|_| word()).collect::<Vec<_>>().join(" ");
                documents.push(doc(kind, id, &name));
            }
        }
        let started = std::time::Instant::now();
        let index = Index::build(documents);
        println!(
            "built {} documents, {} words, in {:?}",
            index.len(),
            index.vocabulary.len(),
            started.elapsed()
        );

        let common = index
            .vocabulary
            .iter()
            .max_by_key(|w| index.postings[index.prefixed(w).start].len())
            .unwrap()
            .clone();
        let mut queries = vec![
            "a".to_owned(),
            "s".to_owned(),
            "ab".to_owned(),
            common.clone(),
        ];
        let long = index
            .vocabulary
            .iter()
            .find(|w| w.len() >= 8)
            .unwrap()
            .clone();
        let mut typo = long.clone();
        typo.replace_range(3..4, "z");
        queries.extend([
            typo,
            format!("{common} {long}"),
            format!("{} {}", &long[..3], &common[..2]),
        ]);
        for query in &queries {
            let started = std::time::Instant::now();
            let rounds = 20;
            let mut sections = Vec::new();
            for _ in 0..rounds {
                sections = index.search(query, &Kind::ALL, 5);
            }
            let totals: Vec<usize> = sections.iter().map(|s| s.total).collect();
            println!(
                "{query:>22}: {:>8.2?} per search, totals {totals:?}",
                started.elapsed() / rounds
            );
        }
    }
}
