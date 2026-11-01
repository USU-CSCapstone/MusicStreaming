//! Playlist edits (`api/openapi.yaml`, `PlaylistOp`; `requirements/playlists.md` §4).
//!
//! The same ops mean the same thing on the server and on a device, so the order they
//! produce is defined once, here. Generic over the item, so the server can carry a stored
//! key with each one and a device a plain track.

use std::fmt;

/// One playlist edit. Positions are zero-based indexes into the playlist as the op finds
/// it, after the ops before it in the same edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op<T> {
    /// Insert `items`, already expanded to playlist items, so the first takes position
    /// `at`. `None` appends. No items, as when an album expands to nothing, changes nothing.
    Add { at: Option<usize>, items: Vec<T> },
    /// Remove the items at `positions`, in any order.
    Remove { positions: Vec<usize> },
    /// Move the items at `positions` together, keeping their relative order, so the first
    /// takes position `at` once the move is done.
    Move { positions: Vec<usize>, at: usize },
}

/// Why an edit was refused. Nothing applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpError {
    /// A remove or move with no positions.
    Empty,
    /// A position, or an `at`, past the end of the playlist at that op.
    OutOfRange(usize),
    /// The same position named twice in one op.
    Duplicate(usize),
}

impl fmt::Display for OpError {
    /// Writes a short message naming the problem and, where there is one, the position.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpError::Empty => f.write_str("op has no positions"),
            OpError::OutOfRange(p) => write!(f, "position {p} is out of range"),
            OpError::Duplicate(p) => write!(f, "position {p} is given twice"),
        }
    }
}

impl std::error::Error for OpError {}

/// `items` after `ops`, each applied to the playlist as the ones before it left it.
///
/// Items are moved, never cloned, so a caller that keys items can tell what is new (it put
/// it in an add) and what moved (its key is out of order). The first bad op fails the
/// whole edit; `items` is consumed either way, so a caller keeps its own copy to fall back
/// on.
pub fn apply_ops<T>(mut items: Vec<T>, ops: Vec<Op<T>>) -> Result<Vec<T>, OpError> {
    for op in ops {
        items = apply_op(items, op)?;
    }
    Ok(items)
}

/// One op applied to `list`; fails without a result if the op is bad.
fn apply_op<T>(mut list: Vec<T>, op: Op<T>) -> Result<Vec<T>, OpError> {
    match op {
        Op::Add { at, items } => {
            let at = at.unwrap_or(list.len());
            check(at, list.len())?;
            list.splice(at..at, items);
            Ok(list)
        }
        Op::Remove { positions } => {
            let drop = selection(positions, list.len())?;
            Ok(without(list, &drop).0)
        }
        Op::Move { positions, at } => {
            let moving = selection(positions, list.len())?;
            let (mut rest, taken) = without(list, &moving);
            check(at, rest.len())?;
            rest.splice(at..at, taken);
            Ok(rest)
        }
    }
}

/// `list` without the items at the ascending `drop` positions, and those items in order.
fn without<T>(list: Vec<T>, drop: &[usize]) -> (Vec<T>, Vec<T>) {
    let mut drop = drop.iter().peekable();
    let mut rest = Vec::with_capacity(list.len());
    let mut taken = Vec::new();
    for (i, item) in list.into_iter().enumerate() {
        if drop.next_if(|&&p| p == i).is_some() {
            taken.push(item);
        } else {
            rest.push(item);
        }
    }
    (rest, taken)
}

/// A remove's or move's positions in playlist order, which is the order a move keeps.
fn selection(mut positions: Vec<usize>, length: usize) -> Result<Vec<usize>, OpError> {
    if positions.is_empty() {
        return Err(OpError::Empty);
    }
    positions.sort_unstable();
    for (i, &p) in positions.iter().enumerate() {
        if p >= length {
            return Err(OpError::OutOfRange(p));
        }
        if i > 0 && positions[i - 1] == p {
            return Err(OpError::Duplicate(p));
        }
    }
    Ok(positions)
}

/// Errors unless `p` is a position from 0 to `max`.
fn check(p: usize, max: usize) -> Result<(), OpError> {
    if p > max {
        return Err(OpError::OutOfRange(p));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The characters of `s`, as playlist items.
    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    /// Applies `ops` to the items spelled by `items`, returning the result as a string.
    fn apply(items: &str, ops: Vec<Op<char>>) -> Result<String, OpError> {
        apply_ops(chars(items), ops).map(|v| v.into_iter().collect())
    }

    /// An add op that inserts the characters of `items` at `at`.
    fn add(at: Option<usize>, items: &str) -> Op<char> {
        Op::Add { at, items: chars(items) }
    }

    /// A remove op for `positions`.
    fn remove(positions: &[usize]) -> Op<char> {
        Op::Remove { positions: positions.to_vec() }
    }

    /// A move op for `positions` to `at`.
    fn mv(positions: &[usize], at: usize) -> Op<char> {
        Op::Move { positions: positions.to_vec(), at }
    }

    /// Twelve distinct items, enough for a scattered selection.
    const TWELVE: &str = "abcdefghijkl";

    #[test]
    fn add_appends_or_inserts() {
        assert_eq!(apply("abc", vec![add(None, "xy")]).unwrap(), "abcxy");
        assert_eq!(apply("abc", vec![add(Some(0), "xy")]).unwrap(), "xyabc");
        assert_eq!(apply("abc", vec![add(Some(3), "x")]).unwrap(), "abcx");
        assert_eq!(apply("", vec![add(None, "x")]).unwrap(), "x");
        assert_eq!(apply("abc", vec![add(None, "a")]).unwrap(), "abca");
    }

    #[test]
    fn add_refuses_past_the_end_but_no_items_is_a_no_op() {
        assert_eq!(apply("abc", vec![add(Some(4), "x")]), Err(OpError::OutOfRange(4)));
        assert_eq!(apply("abc", vec![add(None, "")]).unwrap(), "abc");
        assert_eq!(apply("abc", vec![add(Some(4), "")]), Err(OpError::OutOfRange(4)));
    }

    #[test]
    fn remove_takes_scattered_positions() {
        assert_eq!(apply(TWELVE, vec![remove(&[11, 1, 6])]).unwrap(), "acdefhijk");
        assert_eq!(apply("aba", vec![remove(&[2])]).unwrap(), "ab");
    }

    #[test]
    fn remove_refuses_bad_positions() {
        assert_eq!(apply("abc", vec![remove(&[3])]), Err(OpError::OutOfRange(3)));
        assert_eq!(apply("abc", vec![remove(&[1, 1])]), Err(OpError::Duplicate(1)));
        assert_eq!(apply("abc", vec![remove(&[])]), Err(OpError::Empty));
    }

    #[test]
    fn move_keeps_a_scattered_selection_in_order() {
        // The 2nd, 7th, and 12th, as in `requirements/playlists.md` §4.
        assert_eq!(apply(TWELVE, vec![mv(&[1, 6, 11], 0)]).unwrap(), "bglacdefhijk");
        assert_eq!(apply(TWELVE, vec![mv(&[1, 6, 11], 4)]).unwrap(), "acdebglfhijk");
        assert_eq!(apply(TWELVE, vec![mv(&[11, 1, 6], 0)]).unwrap(), "bglacdefhijk");
    }

    #[test]
    fn move_puts_the_first_moved_item_at_the_position() {
        assert_eq!(apply("abc", vec![mv(&[0], 2)]).unwrap(), "bca");
        assert_eq!(apply("abc", vec![mv(&[0, 1], 1)]).unwrap(), "cab");
        assert_eq!(apply("abc", vec![mv(&[2], 0)]).unwrap(), "cab");
    }

    #[test]
    fn move_refuses_a_position_the_items_would_not_fit_at() {
        assert_eq!(apply("abc", vec![mv(&[0, 1], 2)]), Err(OpError::OutOfRange(2)));
        assert_eq!(apply("abc", vec![mv(&[3], 0)]), Err(OpError::OutOfRange(3)));
        assert_eq!(apply("abc", vec![mv(&[1, 1], 0)]), Err(OpError::Duplicate(1)));
        assert_eq!(apply("abc", vec![mv(&[], 0)]), Err(OpError::Empty));
    }

    #[test]
    fn each_op_sees_the_playlist_the_ones_before_left() {
        let ops = vec![remove(&[0]), add(Some(0), "x"), mv(&[0], 2)];
        assert_eq!(apply("abc", ops).unwrap(), "bcx");
    }

    #[test]
    fn a_later_bad_op_fails_the_whole_edit() {
        let ops = vec![remove(&[0]), remove(&[2])];
        assert_eq!(apply("abc", ops), Err(OpError::OutOfRange(2)));
    }

    #[test]
    fn no_ops_changes_nothing() {
        assert_eq!(apply("abc", vec![]).unwrap(), "abc");
    }
}
