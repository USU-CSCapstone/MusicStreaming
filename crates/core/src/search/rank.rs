//! Where a match stands in its section, so a page can continue after it.
//!
//! A rank is the match's score, then its weight, its folded name's words and its ID: the section's
//! whole order, written out in values rather than index positions. Those values come from the
//! document and the query alone, so a rank still places a page correctly once the index is
//! rebuilt, as a list cursor's sort values do after rows are added: nothing is shown twice, and
//! nothing still there is passed over.

use std::cmp::Reverse;

use super::query::Score;

/// Where a match stands in its section's ranking. Ranks order as the section does, best first.
/// Its parts are plain values, for a caller to hand out in a cursor and read back; one made up
/// rather than read back only chooses where a page starts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rank {
    pub score: Score,
    /// The heavier first.
    pub weight: Reverse<u32>,
    /// The name's folded words. They compare as the index's tie-break compares them, since its
    /// vocabulary is sorted.
    pub words: Vec<String>,
    pub id: i64,
}
