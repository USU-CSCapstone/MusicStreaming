//! Building an index: every distinct word, which documents hold it, and each document's words in
//! tie-break order.

use std::cmp::Ordering;
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
        let mut finishing = self.finishing();
        while !finishing.step(usize::MAX) {}
        finishing.into_index()
    }

    /// The making of the index of every document added, which [`Finishing::step`] does a bounded
    /// amount at a time, as [`finish`](Self::finish) does all at once.
    pub fn finishing(self) -> Finishing {
        let IndexBuilder { numbers, kinds, ids, words, ends } = self;
        let mut vocabulary = vec![Box::<str>::default(); numbers.len()];
        for (word, number) in numbers {
            vocabulary[number as usize] = word;
        }
        let sort = Sorting::new((0..vocabulary.len() as u32).collect());
        Finishing {
            vocabulary,
            text: String::new(),
            word_ends: Vec::new(),
            kinds,
            ids,
            words,
            ends,
            sort,
            ordering: false,
        }
    }
}

/// An index being made, in two sorts and one pass over what they leave: the vocabulary, then the
/// documents in tie-break order. See [`IndexBuilder::finishing`].
pub struct Finishing {
    /// Each distinct word, by the number it was first seen as, until the sort of them is done.
    vocabulary: Vec<Box<str>>,
    /// The sorted words, end to end, and where each ends.
    text: String,
    word_ends: Vec<u32>,
    /// The documents, in the order added, with their words as the numbers first seen and then,
    /// once the vocabulary is sorted, as its indexes.
    kinds: Vec<Kind>,
    ids: Vec<i64>,
    words: Vec<u32>,
    ends: Vec<u32>,
    /// The sort under way: of the vocabulary's numbers, then (`ordering`) of the documents.
    sort: Sorting<u32>,
    ordering: bool,
}

impl Finishing {
    /// Does about `budget` elements' worth of the work, and says whether the sorts are done, which
    /// [`into_index`](Self::into_index) then follows. The call that ends the vocabulary's sort
    /// also lays it out and renumbers the documents' words, a pass of its own that `budget` does
    /// not bound.
    pub fn step(&mut self, budget: usize) -> bool {
        if !self.ordering {
            let vocabulary = &self.vocabulary;
            let less = |&a: &u32, &b: &u32| vocabulary[a as usize].cmp(&vocabulary[b as usize]);
            if self.sort.step(budget, less) {
                self.number_words();
            }
            return false;
        }
        // In tie-break order. Comparing indexes is comparing the words, and cheaper.
        let (kinds, ids, words, ends) = (&self.kinds, &self.ids, &self.words, &self.ends);
        let doc_words = |d: u32| &words[span(ends, d as usize)];
        self.sort.step(budget, |&a, &b| {
            (kinds[a as usize], doc_words(a), ids[a as usize]).cmp(&(
                kinds[b as usize],
                doc_words(b),
                ids[b as usize],
            ))
        })
    }

    /// Lays the sorted vocabulary out as text, renumbers the documents' words by their place in
    /// it, and starts the sort of the documents.
    fn number_words(&mut self) {
        // Sorted, so vocabulary indexes compare as the words do.
        let sorted = std::mem::take(&mut self.sort).items;
        let mut text = String::with_capacity(self.vocabulary.iter().map(|word| word.len()).sum());
        let mut renumber = vec![0; sorted.len()];
        for (w, &number) in sorted.iter().enumerate() {
            renumber[number as usize] = w as u32;
            text.push_str(&self.vocabulary[number as usize]);
            self.word_ends.push(text.len() as u32);
        }
        self.text = text;
        self.vocabulary = Vec::new();
        for word in &mut self.words {
            *word = renumber[*word as usize];
        }
        self.sort = Sorting::new((0..self.kinds.len() as u32).collect());
        self.ordering = true;
    }

    /// The index, once [`step`](Self::step) has said the sorts are done.
    pub fn into_index(self) -> Index {
        let Finishing { text, word_ends, kinds, ids, words, ends, sort, .. } = self;
        let order = sort.items;
        let doc_words = |d: usize| &words[span(&ends, d)];
        let mut index = Index {
            text,
            word_ends,
            kinds: order.iter().map(|&d| kinds[d as usize]).collect(),
            ids: order.iter().map(|&d| ids[d as usize]).collect(),
            words: Vec::with_capacity(words.len()),
            ends: Vec::with_capacity(ends.len()),
            ..Index::default()
        };
        for &d in &order {
            index.words.extend_from_slice(doc_words(d as usize));
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

/// What sorting one element of a run costs, in merged elements, which is what a call's budget
/// counts: about the comparisons it takes, so a call's runs are an eighth as long as its merges.
const RUN_COST: usize = 8;

/// A merge sort that goes on a bounded amount of work at a time, so that sorting hundreds of
/// thousands of elements can be spread over many calls: sorted runs, then merged in pairs until
/// one is left.
#[derive(Default)]
struct Sorting<T> {
    /// The elements: the runs sorted, as far as the first `sorted` elements go, ending at `ends`.
    items: Vec<T>,
    sorted: usize,
    ends: Vec<usize>,
    /// A merge pass under way: the merged runs so far, the pair of `ends` being merged, and the
    /// next element of each of its two runs.
    merged: Vec<T>,
    merged_ends: Vec<usize>,
    pair: usize,
    left: usize,
    right: usize,
}

impl<T: Copy> Sorting<T> {
    /// A sort of `items`, not yet begun.
    fn new(items: Vec<T>) -> Self {
        Sorting {
            merged: Vec::with_capacity(items.len()),
            items,
            sorted: 0,
            ends: Vec::new(),
            merged_ends: Vec::new(),
            pair: 0,
            left: 0,
            right: 0,
        }
    }

    /// Does about `budget` elements' worth of the sort by `less`, and says whether it is done,
    /// in which case the elements are sorted in `items`.
    fn step(&mut self, budget: usize, less: impl Fn(&T, &T) -> Ordering) -> bool {
        let mut work = 0;
        while self.sorted < self.items.len() && work < budget {
            let end = self
                .sorted
                .saturating_add((budget - work).div_ceil(RUN_COST))
                .min(self.items.len());
            self.items[self.sorted..end].sort_unstable_by(&less);
            self.ends.push(end);
            work = work.saturating_add((end - self.sorted).saturating_mul(RUN_COST));
            self.sorted = end;
            if self.sorted == self.items.len() {
                self.begin_pair();
            }
        }
        if self.sorted < self.items.len() {
            return false;
        }
        while self.ends.len() > 1 && work < budget {
            let left_end = self.ends[self.pair];
            let right_end = self.ends.get(self.pair + 1).copied().unwrap_or(left_end);
            let from_left = self.left < left_end
                && (self.right >= right_end
                    || less(&self.items[self.left], &self.items[self.right]) != Ordering::Greater);
            if from_left {
                self.merged.push(self.items[self.left]);
                self.left += 1;
            } else if self.right < right_end {
                self.merged.push(self.items[self.right]);
                self.right += 1;
            } else {
                self.merged_ends.push(self.merged.len());
                self.pair += 2;
                if self.pair < self.ends.len() {
                    self.begin_pair();
                } else {
                    std::mem::swap(&mut self.items, &mut self.merged);
                    self.merged.clear();
                    self.ends = std::mem::take(&mut self.merged_ends);
                    self.pair = 0;
                    self.begin_pair();
                }
            }
            work += 1;
        }
        self.ends.len() <= 1
    }

    /// Points the merge at the start of the pair of runs at `pair`.
    fn begin_pair(&mut self) {
        self.left = if self.pair == 0 { 0 } else { self.ends[self.pair - 1] };
        self.right = self.ends.get(self.pair).copied().unwrap_or(0);
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
