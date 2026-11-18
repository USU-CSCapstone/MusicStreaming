//! Lyrics, ready to search a line at a time (`design/search.md` §2): what a line is, and the
//! index of every track's lines, kept small, since lyrics outweigh names many times over.
//!
//! A track's lyrics are its lines in order: a synced line each, or plain lyrics split at line
//! breaks ([`plain_lines`]). A line is folded into words as a name is ([`words`]). A line with no
//! words, and a section label alone on its line in brackets such as `[Chorus]`, are not indexed;
//! nor is a line again where the track already has it, since a track is one result however often
//! its words recur. Each line indexed keeps its place among the track's lines, for the lines
//! around it to be read back from where the lyrics are stored.
//!
//! The index holds, for each word, the lines it is in, and for each line, its words in order:
//!
//! - **words are numbered by how often they occur**, most first, so the commonest, which most
//!   lines are made of, are the smallest numbers;
//! - **a line's words are those numbers in order, a few bytes each and most one** (variable
//!   length, seven bits to a byte);
//! - **a word's lines are ascending, kept as the gaps between them**, which are small for a
//!   common word, in the same variable-length bytes;
//! - **a line's track is found, not stored**: lines are in track order, and each track keeps
//!   where its lines start.

use std::collections::{HashMap, HashSet};

use super::words;

/// The lines of plain lyrics, as their line breaks divide them.
pub fn plain_lines(plain: &str) -> impl Iterator<Item = &str> {
    plain.lines()
}

/// Whether `line` is a section label alone on its line, such as `[Chorus]` or `[Verse 2]`.
fn is_label(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 2 && line.starts_with('[') && line.ends_with(']')
}

/// A track's ID and its distinct lines, each with its place among them, folded into words.
type TrackLines = (i64, Vec<(u16, Vec<String>)>);

/// Every track's lyrics by their lines, ready to search.
#[derive(Debug, Default)]
pub struct LyricIndex {
    /// Every distinct word, most frequent first: a word's number is its place here.
    vocabulary: Vec<String>,
    /// The word numbers in alphabetical order, for finding words by prefix or with typos.
    alphabetical: Vec<u32>,
    /// Each word's lines, ascending, as gaps in variable-length bytes:
    /// `postings[posting_starts[w]..posting_starts[w + 1]]`.
    postings: Vec<u8>,
    posting_starts: Vec<u32>,
    /// Each line's word numbers in order, in variable-length bytes:
    /// `line_words[word_starts[l]..word_starts[l + 1]]`.
    line_words: Vec<u8>,
    word_starts: Vec<u32>,
    /// Each line's place among its track's lines as given.
    line_numbers: Vec<u16>,
    /// The tracks, by ID, and where each one's lines start: `track_starts[t]..track_starts[t + 1]`.
    track_ids: Vec<i64>,
    track_starts: Vec<u32>,
}

impl LyricIndex {
    /// An index of `tracks`' lyrics, each a track's ID and its lines in order. A track with no
    /// line to index is left out; past 65,535 lines, the rest of a track's are.
    pub fn build<L: AsRef<str>>(tracks: impl IntoIterator<Item = (i64, Vec<L>)>) -> LyricIndex {
        // Each track's distinct lines, folded, with their places.
        let mut folded: Vec<TrackLines> = Vec::new();
        for (id, lines) in tracks {
            let mut seen: HashSet<Vec<String>> = HashSet::new();
            let mut kept = Vec::new();
            for (at, line) in lines.iter().enumerate() {
                let Ok(at) = u16::try_from(at) else { break };
                let line = line.as_ref();
                if is_label(line) {
                    continue;
                }
                let words = words(line);
                if !words.is_empty() && seen.insert(words.clone()) {
                    kept.push((at, words));
                }
            }
            if !kept.is_empty() {
                folded.push((id, kept));
            }
        }
        folded.sort_unstable_by_key(|(id, _)| *id);

        // Words numbered by how often they occur, ties by the word, so the numbering is the same
        // wherever the index is built.
        let mut counts: HashMap<&str, u64> = HashMap::new();
        for (_, lines) in &folded {
            for (_, words) in lines {
                for word in words {
                    *counts.entry(word.as_str()).or_default() += 1;
                }
            }
        }
        let mut vocabulary: Vec<(&str, u64)> = counts.into_iter().collect();
        vocabulary.sort_unstable_by(|(a, x), (b, y)| y.cmp(x).then(a.cmp(b)));
        let number: HashMap<&str, u32> =
            vocabulary.iter().enumerate().map(|(n, (word, _))| (*word, n as u32)).collect();
        let mut alphabetical: Vec<u32> = (0..vocabulary.len() as u32).collect();
        alphabetical.sort_unstable_by_key(|&w| vocabulary[w as usize].0);

        let mut index = LyricIndex::default();
        let mut lines_of: Vec<Vec<u32>> = vec![Vec::new(); vocabulary.len()];
        for (id, lines) in &folded {
            index.track_ids.push(*id);
            index.track_starts.push(index.line_numbers.len() as u32);
            for (at, words) in lines {
                let line = index.line_numbers.len() as u32;
                index.line_numbers.push(*at);
                index.word_starts.push(index.line_words.len() as u32);
                let mut last = None;
                for word in words {
                    let w = number[word.as_str()];
                    put_varint(&mut index.line_words, w);
                    // A word twice in a line is one posting.
                    if last != Some(w) && lines_of[w as usize].last() != Some(&line) {
                        lines_of[w as usize].push(line);
                    }
                    last = Some(w);
                }
            }
        }
        index.track_starts.push(index.line_numbers.len() as u32);
        index.word_starts.push(index.line_words.len() as u32);
        for lines in lines_of {
            index.posting_starts.push(index.postings.len() as u32);
            let mut previous = 0;
            for line in lines {
                put_varint(&mut index.postings, line - previous);
                previous = line;
            }
        }
        index.posting_starts.push(index.postings.len() as u32);
        index.vocabulary = vocabulary.into_iter().map(|(word, _)| word.to_owned()).collect();
        index.alphabetical = alphabetical;
        index.shrink();
        index
    }

    /// Lets go of what building left spare.
    fn shrink(&mut self) {
        self.vocabulary.shrink_to_fit();
        self.alphabetical.shrink_to_fit();
        self.postings.shrink_to_fit();
        self.posting_starts.shrink_to_fit();
        self.line_words.shrink_to_fit();
        self.word_starts.shrink_to_fit();
        self.line_numbers.shrink_to_fit();
        self.track_ids.shrink_to_fit();
        self.track_starts.shrink_to_fit();
    }

    /// How many bytes the index holds, all told, for keeping it within its budget.
    pub fn bytes(&self) -> usize {
        let words: usize = self.vocabulary.iter().map(|word| word.capacity()).sum();
        words
            + self.vocabulary.capacity() * size_of::<String>()
            + self.alphabetical.capacity() * size_of::<u32>()
            + self.postings.capacity()
            + self.posting_starts.capacity() * size_of::<u32>()
            + self.line_words.capacity()
            + self.word_starts.capacity() * size_of::<u32>()
            + self.line_numbers.capacity() * size_of::<u16>()
            + self.track_ids.capacity() * size_of::<i64>()
            + self.track_starts.capacity() * size_of::<u32>()
    }

    /// What each part of the index holds, in bytes, for the benchmark to say where they go.
    #[cfg(test)]
    pub(super) fn parts(&self) -> [(&'static str, usize); 5] {
        let words: usize = self.vocabulary.iter().map(|word| word.capacity()).sum();
        let vocabulary = words
            + self.vocabulary.capacity() * size_of::<String>()
            + self.alphabetical.capacity() * size_of::<u32>();
        [
            ("postings", self.postings.capacity() + self.posting_starts.capacity() * 4),
            ("line words", self.line_words.capacity()),
            ("line starts", self.word_starts.capacity() * 4),
            (
                "line places and tracks",
                self.line_numbers.capacity() * 2
                    + self.track_ids.capacity() * 8
                    + self.track_starts.capacity() * 4,
            ),
            ("vocabulary", vocabulary),
        ]
    }

    /// How many tracks have lines indexed.
    pub fn tracks(&self) -> usize {
        self.track_ids.len()
    }

    /// How many lines are indexed.
    pub fn lines(&self) -> usize {
        self.line_numbers.len()
    }

    /// The number of `word`, if any line has it.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by lyric matching, built next"))]
    pub(super) fn word(&self, word: &str) -> Option<u32> {
        let at =
            self.alphabetical.partition_point(|&w| self.vocabulary[w as usize].as_str() < word);
        let &w = self.alphabetical.get(at)?;
        (self.vocabulary[w as usize] == word).then_some(w)
    }

    /// The lines word `w` is in, ascending.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by lyric matching, built next"))]
    pub(super) fn lines_of(&self, w: u32) -> impl Iterator<Item = u32> + '_ {
        let range =
            self.posting_starts[w as usize] as usize..self.posting_starts[w as usize + 1] as usize;
        let mut previous = 0;
        Varints(&self.postings[range]).map(move |gap| {
            previous += gap;
            previous
        })
    }

    /// The word numbers of line `l`, in order.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by lyric matching, built next"))]
    pub(super) fn words_of(&self, l: u32) -> impl Iterator<Item = u32> + '_ {
        let range =
            self.word_starts[l as usize] as usize..self.word_starts[l as usize + 1] as usize;
        Varints(&self.line_words[range])
    }

    /// The track line `l` is of, by its ID, and the line's place among the track's lines.
    #[cfg_attr(not(test), expect(dead_code, reason = "read by lyric matching, built next"))]
    pub(super) fn place(&self, l: u32) -> (i64, u16) {
        let t = self.track_starts.partition_point(|&start| start <= l) - 1;
        (self.track_ids[t], self.line_numbers[l as usize])
    }
}

/// Appends `n` in variable-length bytes: seven bits to a byte, low first, the high bit set on
/// every byte but the last.
fn put_varint(out: &mut Vec<u8>, mut n: u32) {
    while n >= 0x80 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}

/// The numbers in bytes [`put_varint`] wrote.
#[cfg_attr(not(test), expect(dead_code, reason = "read by lyric matching, built next"))]
struct Varints<'a>(&'a [u8]);

impl Iterator for Varints<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let mut n = 0u32;
        let mut shift = 0;
        loop {
            let (&byte, rest) = self.0.split_first()?;
            self.0 = rest;
            n |= u32::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Some(n);
            }
            shift += 7;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index's words, as text, of each line `word` is in, and where each is.
    fn found(index: &LyricIndex, word: &str) -> Vec<(i64, u16, Vec<String>)> {
        let Some(w) = index.word(word) else { return Vec::new() };
        index
            .lines_of(w)
            .map(|l| {
                let (track, at) = index.place(l);
                let words = index.words_of(l).map(|w| index.vocabulary[w as usize].clone());
                (track, at, words.collect())
            })
            .collect()
    }

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|&word| word.to_owned()).collect()
    }

    #[test]
    fn lines_are_indexed_by_their_words_with_their_places() {
        let index = LyricIndex::build([
            (7, vec!["Say it ain't so", "I will not go", "Say it ain’t so!"]),
            (3, vec!["", "[Chorus]", "so long", "  "]),
        ]);
        assert_eq!((index.tracks(), index.lines()), (2, 3));
        assert_eq!(
            found(&index, "so"),
            [(3, 2, strings(&["so", "long"])), (7, 0, strings(&["say", "it", "aint", "so"])),],
            "the repeated line once, at its first place; the blank and the label not at all"
        );
        assert_eq!(found(&index, "go"), [(7, 1, strings(&["i", "will", "not", "go"]))]);
        assert_eq!(found(&index, "chorus"), []);
        assert_eq!(found(&index, "nowhere"), []);
    }

    #[test]
    fn the_commonest_words_are_the_smallest_numbers() {
        let index = LyricIndex::build([
            (1, vec!["love love me do", "you know I love you"]),
            (2, vec!["love is all", "all you need is love"]),
        ]);
        assert_eq!(index.word("love"), Some(0));
        // You three times, then all and is twice each, ties by the word.
        assert_eq!(index.vocabulary[1..4], ["you", "all", "is"]);
        // A word twice in a line is that line once.
        assert_eq!(found(&index, "love").len(), 4);
    }

    #[test]
    fn a_track_with_nothing_to_index_is_left_out() {
        let index = LyricIndex::build([(1, vec!["", "[Instrumental]"]), (2, vec!["la"])]);
        assert_eq!((index.tracks(), index.lines()), (1, 1));
        assert!(LyricIndex::build(Vec::<(i64, Vec<&str>)>::new()).bytes() < 64);
    }

    #[test]
    fn numbers_read_back_from_their_bytes() {
        let numbers = [0, 1, 127, 128, 300, 16_383, 16_384, u32::MAX];
        let mut bytes = Vec::new();
        for n in numbers {
            put_varint(&mut bytes, n);
        }
        assert_eq!(Varints(&bytes).collect::<Vec<_>>(), numbers);
        assert_eq!(bytes.len(), 1 + 1 + 1 + 2 + 2 + 2 + 3 + 5);
    }

    #[test]
    fn plain_lyrics_are_lines_at_their_breaks() {
        assert_eq!(
            plain_lines("one\ntwo\r\n\nthree").collect::<Vec<_>>(),
            ["one", "two", "", "three"]
        );
    }
}
