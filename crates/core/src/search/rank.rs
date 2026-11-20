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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rank {
    pub(super) score: Score,
    /// The heavier first.
    pub(super) weight: Reverse<u32>,
    /// The name's folded words. They compare as the index's tie-break compares them, since its
    /// vocabulary is sorted.
    pub(super) words: Vec<String>,
    pub(super) id: i64,
}

impl Rank {
    /// The rank as bytes, for a client to hand back as a cursor.
    pub fn to_bytes(&self) -> Vec<u8> {
        let Score { inexact, typos, skipped, prefixes, extra_words } = self.score;
        let mut bytes = vec![u8::from(inexact), u8::from(skipped)];
        for n in [typos, prefixes, extra_words, self.weight.0] {
            bytes.extend(n.to_be_bytes());
        }
        bytes.extend(self.id.to_be_bytes());
        for word in &self.words {
            let len = u32::try_from(word.len()).expect("a word is under 4 GiB");
            bytes.extend(len.to_be_bytes());
            bytes.extend(word.as_bytes());
        }
        bytes
    }

    /// The rank [`Rank::to_bytes`] wrote, or `None` for anything else. A rank made up rather
    /// than read back only chooses where a page starts.
    pub fn from_bytes(bytes: &[u8]) -> Option<Rank> {
        let flag = |byte: u8| match byte {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        let (&[inexact, skipped], rest) = bytes.split_first_chunk::<2>()?;
        let (typos, rest) = rest.split_first_chunk::<4>()?;
        let (prefixes, rest) = rest.split_first_chunk::<4>()?;
        let (extra_words, rest) = rest.split_first_chunk::<4>()?;
        let (weight, rest) = rest.split_first_chunk::<4>()?;
        let (id, mut rest) = rest.split_first_chunk::<8>()?;
        let mut words = Vec::new();
        while !rest.is_empty() {
            let (len, tail) = rest.split_first_chunk::<4>()?;
            let (word, tail) =
                tail.split_at_checked(usize::try_from(u32::from_be_bytes(*len)).ok()?)?;
            words.push(String::from_utf8(word.to_vec()).ok()?);
            rest = tail;
        }
        Some(Rank {
            score: Score {
                inexact: flag(inexact)?,
                typos: u32::from_be_bytes(*typos),
                skipped: flag(skipped)?,
                prefixes: u32::from_be_bytes(*prefixes),
                extra_words: u32::from_be_bytes(*extra_words),
            },
            weight: Reverse(u32::from_be_bytes(*weight)),
            words,
            id: i64::from_be_bytes(*id),
        })
    }
}
