//! Building an index: every distinct word, which documents hold it, and each document's words in
//! tie-break order.

use std::collections::{HashMap, HashSet};

use super::{Document, Entry, Index, Kind, words};

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
}
