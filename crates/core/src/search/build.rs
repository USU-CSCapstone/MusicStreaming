//! Building an index: every distinct word, which documents hold it, and each document's words in
//! tie-break order.

use std::collections::HashMap;

use super::words::fold;
use super::{Document, Index, Kind, span};

/// An [`Index`] being built one document at a time, so the names it is built from need never be
/// held all at once: each is folded into numbered words as it is added. [`finish`](Self::finish)
/// makes the index.
#[derive(Debug, Default)]
pub struct IndexBuilder {
    /// Each distinct word so far, numbered in the order first seen.
    numbers: HashMap<Box<str>, u32>,
    /// Each document's kind and ID, in the order added.
    kinds: Vec<Kind>,
    ids: Vec<i64>,
    /// Each document's words by those numbers: `words[span(&ends, d)]`.
    words: Vec<u32>,
    ends: Vec<u32>,
}

impl IndexBuilder {
    /// Adds `document`, its name folded into words as [`words`](super::words) folds it.
    pub fn add(&mut self, document: Document) {
        fold(&document.name, |word| {
            let number = match self.numbers.get(word) {
                Some(&number) => number,
                None => {
                    let next = self.numbers.len() as u32;
                    self.numbers.insert(word.into(), next);
                    next
                }
            };
            self.words.push(number);
        });
        self.ends.push(self.words.len() as u32);
        self.kinds.push(document.kind);
        self.ids.push(document.id);
    }

    /// The index of every document added, ready to search.
    pub fn finish(self) -> Index {
        let IndexBuilder { numbers, kinds, ids, mut words, ends } = self;

        // Sorted, so vocabulary indexes compare as the words do. Map order never shows.
        let mut vocabulary: Vec<(Box<str>, u32)> = numbers.into_iter().collect();
        vocabulary.sort_unstable();
        let mut text = String::with_capacity(vocabulary.iter().map(|(word, _)| word.len()).sum());
        let mut word_ends = Vec::with_capacity(vocabulary.len());
        let mut renumber = vec![0; vocabulary.len()];
        for (w, (word, number)) in vocabulary.into_iter().enumerate() {
            renumber[number as usize] = w as u32;
            text.push_str(&word);
            word_ends.push(text.len() as u32);
        }
        for word in &mut words {
            *word = renumber[*word as usize];
        }
        drop(renumber);

        // In tie-break order. Comparing indexes is comparing the words, and cheaper.
        let doc_words = |d: usize| &words[span(&ends, d)];
        let mut order: Vec<usize> = (0..kinds.len()).collect();
        order.sort_unstable_by(|&a, &b| {
            (kinds[a], doc_words(a), ids[a]).cmp(&(kinds[b], doc_words(b), ids[b]))
        });
        let mut index = Index {
            text,
            word_ends,
            kinds: order.iter().map(|&d| kinds[d]).collect(),
            ids: order.iter().map(|&d| ids[d]).collect(),
            words: Vec::with_capacity(words.len()),
            ends: Vec::with_capacity(ends.len()),
            ..Index::default()
        };
        for &d in &order {
            index.words.extend_from_slice(doc_words(d));
            index.ends.push(index.words.len() as u32);
        }
        drop((order, kinds, ids, words, ends));

        // Each word's documents, counted first so that they fill one array, each document once.
        let distinct = |words: &[u32], i: usize| !words[..i].contains(&words[i]);
        let mut next = vec![0; index.vocabulary_len()];
        for d in 0..index.ids.len() {
            let words = index.document_words(d);
            for i in (0..words.len()).filter(|&i| distinct(words, i)) {
                next[words[i] as usize] += 1;
            }
        }
        let mut total = 0;
        for slot in &mut next {
            (*slot, total) = (total, total + *slot);
        }
        let mut postings = vec![0; total as usize];
        for d in 0..index.ids.len() {
            let words = index.document_words(d);
            for i in (0..words.len()).filter(|&i| distinct(words, i)) {
                let slot = &mut next[words[i] as usize];
                postings[*slot as usize] = d as u32;
                *slot += 1;
            }
        }
        // Each word's slot has moved to the end of its documents.
        index.postings = postings;
        index.posting_ends = next;
        index
    }
}

impl Index {
    /// An index of `documents`, ready to search. Each document's name is folded into words as
    /// [`words`](super::words) folds it.
    pub fn build(documents: impl IntoIterator<Item = Document>) -> Index {
        let mut builder = IndexBuilder::default();
        for document in documents {
            builder.add(document);
        }
        builder.finish()
    }
}
