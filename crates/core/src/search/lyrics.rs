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
//! The index holds, for each word, the lines it is in, and for each line, how many words it has,
//! but not which, which would double it (`design/search.md` §2.5): matching ranks the lines it
//! finds by what the postings say, and reads the text of only the best few back to see whether
//! they hold the query's words in order.
//!
//! - **a word's lines are ascending, kept as the gaps between them**, which are small for a
//!   common word, in variable-length bytes (seven bits to a byte), most of them one;
//! - **a line's track is found, not stored**: lines are kept a track at a time, and each track keeps
//!   where its lines start.

use std::collections::{HashMap, HashSet};

use super::words;

/// The lines of plain lyrics, as their line breaks divide them.
pub fn plain_lines(plain: &str) -> impl Iterator<Item = &str> {
    plain.lines()
}

/// Whether `line` is a line of lyrics, as the index and what shows a match count them: not blank,
/// nor a section label alone on its line.
pub fn is_lyric_line(line: &str) -> bool {
    !is_label(line) && !words(line).is_empty()
}

/// Whether `line` is a section label alone on its line, such as `[Chorus]` or `[Verse 2]`.
pub(super) fn is_label(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 2 && line.starts_with('[') && line.ends_with(']')
}

/// Every track's lyrics by their lines, ready to search.
#[derive(Debug, Default)]
pub struct LyricIndex {
    /// Every distinct word, sorted: a word's number is its place here.
    vocabulary: Vec<String>,
    /// Each word's lines, ascending, as gaps in variable-length bytes:
    /// `postings[posting_starts[w]..posting_starts[w + 1]]`.
    postings: Vec<u8>,
    posting_starts: Vec<u32>,
    /// Each line's place among its track's lines as given.
    line_numbers: Vec<u16>,
    /// Each line's words, counted, up to 255.
    line_lengths: Vec<u8>,
    /// The tracks, by ID, and where each one's lines start: `track_starts[t]..track_starts[t + 1]`.
    track_ids: Vec<i64>,
    track_starts: Vec<u32>,
}

/// A lyric index being built a track at a time, so that what building holds beyond the index is
/// one track's lines, never every track's: the lyrics can be read a batch at a time and dropped.
#[derive(Debug, Default)]
pub struct LyricIndexBuilder {
    /// The index so far, but for its vocabulary and postings, kept below until they are sorted.
    index: LyricIndex,
    /// Each word's number as it was first met, and its lines as gaps, and its last line.
    numbers: HashMap<String, u32>,
    postings: Vec<Vec<u8>>,
    last_lines: Vec<u32>,
}

impl LyricIndexBuilder {
    /// Adds track `id`'s lyrics, its lines in order. A track with no line to index is left out;
    /// past 65,535 lines, the rest of a track's are. Each track is added once, in any order.
    pub fn add<L: AsRef<str>>(&mut self, id: i64, lines: impl IntoIterator<Item = L>) {
        let start = self.index.line_numbers.len() as u32;
        let mut seen: HashSet<Vec<u32>> = HashSet::new();
        for (at, line) in lines.into_iter().enumerate() {
            let Ok(at) = u16::try_from(at) else { break };
            let line = line.as_ref();
            if is_label(line) {
                continue;
            }
            let words: Vec<u32> = words(line).into_iter().map(|word| self.number(word)).collect();
            if words.is_empty() || seen.contains(&words) {
                continue;
            }
            let l = self.index.line_numbers.len() as u32;
            self.index.line_numbers.push(at);
            self.index.line_lengths.push(u8::try_from(words.len()).unwrap_or(u8::MAX));
            for &w in &words {
                let last = &mut self.last_lines[w as usize];
                // A word twice in a line is one posting.
                if *last != l {
                    let gap = if *last == NONE { l } else { l - *last };
                    put_varint(&mut self.postings[w as usize], gap);
                    *last = l;
                }
            }
            seen.insert(words);
        }
        if self.index.line_numbers.len() as u32 > start {
            self.index.track_ids.push(id);
            self.index.track_starts.push(start);
        }
    }

    /// The number of `word`, given it if it is new.
    fn number(&mut self, word: String) -> u32 {
        if let Some(&w) = self.numbers.get(&word) {
            return w;
        }
        let w = self.postings.len() as u32;
        self.numbers.insert(word, w);
        self.postings.push(Vec::new());
        self.last_lines.push(NONE);
        w
    }

    /// The index of every track added.
    pub fn finish(self) -> LyricIndex {
        let LyricIndexBuilder { mut index, numbers, mut postings, last_lines: _ } = self;
        // Sorted, so words are found by prefix in one range, as a name's are.
        let mut vocabulary: Vec<(String, u32)> = numbers.into_iter().collect();
        vocabulary.sort_unstable();
        for (_, w) in &vocabulary {
            index.posting_starts.push(index.postings.len() as u32);
            index.postings.extend_from_slice(&std::mem::take(&mut postings[*w as usize]));
        }
        index.posting_starts.push(index.postings.len() as u32);
        index.track_starts.push(index.line_numbers.len() as u32);
        index.vocabulary = vocabulary.into_iter().map(|(word, _)| word).collect();
        index.shrink();
        index
    }
}

/// No line yet, as a word's last line.
const NONE: u32 = u32::MAX;

impl LyricIndex {
    /// An index of `tracks`' lyrics, each a track's ID and its lines in order, as
    /// [`LyricIndexBuilder`] builds it.
    pub fn build<L: AsRef<str>>(tracks: impl IntoIterator<Item = (i64, Vec<L>)>) -> LyricIndex {
        let mut builder = LyricIndexBuilder::default();
        for (id, lines) in tracks {
            builder.add(id, lines);
        }
        builder.finish()
    }

    /// Lets go of what building left spare.
    fn shrink(&mut self) {
        self.vocabulary.shrink_to_fit();
        self.postings.shrink_to_fit();
        self.posting_starts.shrink_to_fit();
        self.line_numbers.shrink_to_fit();
        self.line_lengths.shrink_to_fit();
        self.track_ids.shrink_to_fit();
        self.track_starts.shrink_to_fit();
    }

    /// How many bytes the index holds, all told, for keeping it within its budget.
    pub fn bytes(&self) -> usize {
        let words: usize = self.vocabulary.iter().map(|word| word.capacity()).sum();
        words
            + self.vocabulary.capacity() * size_of::<String>()
            + self.postings.capacity()
            + self.posting_starts.capacity() * size_of::<u32>()
            + self.line_numbers.capacity() * size_of::<u16>()
            + self.line_lengths.capacity()
            + self.track_ids.capacity() * size_of::<i64>()
            + self.track_starts.capacity() * size_of::<u32>()
    }

    /// What each part of the index holds, in bytes, for the benchmark to say where they go.
    #[cfg(test)]
    pub(super) fn parts(&self) -> [(&'static str, usize); 5] {
        let words: usize = self.vocabulary.iter().map(|word| word.capacity()).sum();
        let vocabulary = words + self.vocabulary.capacity() * size_of::<String>();
        [
            ("postings", self.postings.capacity() + self.posting_starts.capacity() * 4),
            ("line lengths", self.line_lengths.capacity()),
            ("line places", self.line_numbers.capacity() * 2),
            ("tracks", self.track_ids.capacity() * 8 + self.track_starts.capacity() * 4),
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

    /// Every distinct word, sorted.
    pub(super) fn vocabulary(&self) -> &[String] {
        &self.vocabulary
    }

    /// The number of `word`, if any line has it.
    #[cfg(test)]
    pub(super) fn word(&self, word: &str) -> Option<u32> {
        self.vocabulary.binary_search_by(|w| w.as_str().cmp(word)).ok().map(|w| w as u32)
    }

    /// The lines word `w` is in, ascending.
    pub(super) fn lines_of(&self, w: u32) -> impl Iterator<Item = u32> + '_ {
        let range =
            self.posting_starts[w as usize] as usize..self.posting_starts[w as usize + 1] as usize;
        let mut previous = 0;
        Varints(&self.postings[range]).map(move |gap| {
            previous += gap;
            previous
        })
    }

    /// How many words line `l` has, up to 255.
    pub(super) fn length(&self, l: u32) -> u8 {
        self.line_lengths[l as usize]
    }

    /// The track line `l` is of, by its ID, and the line's place among the track's lines.
    #[cfg(test)]
    pub(super) fn place(&self, l: u32) -> (i64, u16) {
        self.tracks_from(0).place(l)
    }

    /// A walk through the tracks, from the first, for lines asked for in ascending order.
    pub(super) fn tracks_from(&self, t: usize) -> Tracks<'_> {
        Tracks { index: self, t }
    }
}

/// A walk through an index's tracks for its lines in ascending order: each line's track is found
/// from the last one's, so a whole walk reads each track once.
pub(super) struct Tracks<'a> {
    index: &'a LyricIndex,
    t: usize,
}

impl Tracks<'_> {
    /// The track line `l` is of, by its ID, and the line's place among the track's lines. Lines
    /// are asked for in ascending order.
    pub(super) fn place(&mut self, l: u32) -> (i64, u16) {
        let starts = &self.index.track_starts;
        if starts[self.t + 1] <= l {
            // Galloping, as for postings: the next line's track is usually this one or near.
            let rest = &starts[self.t + 1..];
            let mut step = 1;
            while step < rest.len() && rest[step] <= l {
                step *= 2;
            }
            self.t += rest[..(step + 1).min(rest.len())].partition_point(|&start| start <= l);
        }
        (self.index.track_ids[self.t], self.index.line_numbers[l as usize])
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

    /// Each line `word` is in: its track, its place, and how many words it has.
    fn found(index: &LyricIndex, word: &str) -> Vec<(i64, u16, u8)> {
        let Some(w) = index.word(word) else { return Vec::new() };
        index
            .lines_of(w)
            .map(|l| {
                let (track, at) = index.place(l);
                (track, at, index.length(l))
            })
            .collect()
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
            [(7, 0, 4), (3, 2, 2)],
            "the repeated line once, at its first place; the blank and the label not at all"
        );
        assert_eq!(found(&index, "go"), [(7, 1, 4)]);
        assert_eq!(found(&index, "aint"), [(7, 0, 4)], "folded as a name is");
        assert_eq!(found(&index, "chorus"), []);
        assert_eq!(found(&index, "nowhere"), []);
    }

    #[test]
    fn a_word_twice_in_a_line_is_that_line_once_and_counts_twice() {
        let index = LyricIndex::build([(1, vec!["love love me do"]), (2, vec!["love is all"])]);
        assert_eq!(found(&index, "love"), [(1, 0, 4), (2, 0, 3)]);
        assert_eq!(index.vocabulary, ["all", "do", "is", "love", "me"], "sorted");
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
    fn blanks_and_labels_are_not_lines() {
        assert!(is_lyric_line("Say it ain't so") && is_lyric_line("[Chorus] la la"));
        assert!(!is_lyric_line("") && !is_lyric_line("  ") && !is_lyric_line("?!"));
        assert!(!is_lyric_line("[Chorus]") && !is_lyric_line(" [Verse 2] "));
    }

    #[test]
    fn plain_lyrics_are_lines_at_their_breaks() {
        assert_eq!(
            plain_lines("one\ntwo\r\n\nthree").collect::<Vec<_>>(),
            ["one", "two", "", "three"]
        );
    }
}
