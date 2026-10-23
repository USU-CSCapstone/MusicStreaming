//! Search matching and ranking (`requirements/search.md` §3–§4, `design/general.md` §3).
//!
//! One implementation for the server and, later, the device, so both return the same results
//! in the same order. Pure and deterministic: nothing here depends on the platform, the locale,
//! or hash order.
//!
//! Names are folded into words: compatibility forms written plainly (`ﬁ` is `fi`, fullwidth `Ａ`
//! is `a`), accents dropped, case folded with final `ς` as `σ`, apostrophes joining (so `aint` is
//! `Ain't`), anything else that is not a letter or digit separating (so `™` too, not `tm`).
//! Every word of the query must match a word of the name, in any order and two perhaps the same
//! one, in one of three ways:
//!
//! - **exactly**;
//! - **as a prefix**, so results appear from the first character typed;
//! - **with typos**: one for a word of 4 to 7 characters, two from 8, counting a transposed
//!   pair as one. The first character must be right, which keeps the words to try few enough
//!   to meet the time budget, and is the letter people mistype least.
//!
//! The one exception is a leading article in a query of more than one word, which may go
//! unmatched, so `the beatles` finds Beatles.
//!
//! Matches rank by, in order: the whole name matching exactly, a leading article optional on
//! either side (the guardrail that keeps an exact match from being buried); fewer typos; the
//! query's leading article matched, so typing `the` narrows toward names that have it; fewer words
//! matched only as a prefix; fewer words in the name beyond those the query matched. Ties break
//! by kind (tracks, then albums, then artists), then folded name, then ID.

mod build;
mod distance;
#[cfg(test)]
mod full_scale;
mod query;
#[cfg(test)]
mod tests;
mod words;

pub use words::words;

/// What a name names. The order is the tie-break: the more specific thing first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Track,
    Album,
    Artist,
}

impl Kind {
    /// Every kind, in tie-break order.
    pub const ALL: [Kind; 3] = [Kind::Track, Kind::Album, Kind::Artist];
}

/// A thing to find by its name. The constructors are the rule for what is searched, so the
/// server and a device index the same corpus (`requirements/search.md` §2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    kind: Kind,
    id: i64,
    name: String,
}

impl Document {
    /// A track, by its title.
    pub fn from_track(id: i64, title: String) -> Document {
        Document { kind: Kind::Track, id, name: title }
    }

    /// An album, by its title. The unknown album, which has none, is not searched.
    pub fn from_album(id: i64, title: Option<String>) -> Option<Document> {
        title.map(|name| Document { kind: Kind::Album, id, name })
    }

    /// An artist, by their name. The unknown artist, who has none, is not searched.
    pub fn from_artist(id: i64, name: Option<String>) -> Option<Document> {
        name.map(|name| Document { kind: Kind::Artist, id, name })
    }
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

/// Names by their words, ready to search.
#[derive(Debug, Default)]
pub struct Index {
    /// Every distinct word, sorted.
    vocabulary: Vec<String>,
    /// For each word, the documents holding it, ascending.
    postings: Vec<Vec<u32>>,
    /// Documents in tie-break order: by kind, then folded name, then ID.
    documents: Vec<Entry>,
    /// Each document's words as vocabulary indexes, in name order:
    /// `words[starts[d]..starts[d + 1]]`.
    words: Vec<u32>,
    starts: Vec<u32>,
}

#[derive(Debug)]
struct Entry {
    kind: Kind,
    id: i64,
}
