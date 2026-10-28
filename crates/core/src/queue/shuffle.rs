//! Shuffle, unshuffle and the random numbers behind them (`requirements/queue.md` §4).
//!
//! A shuffle is decided by its seed alone, so every device and the server compute the same order,
//! natively and in WebAssembly. The generator is written here, not taken from a crate, because
//! nothing that decides an order may change under a dependency update.

use super::{Queue, Shuffled};

/// SplitMix64, a 64-bit generator with a one-word state, and the same on every platform.
struct SplitMix64(u64);

impl SplitMix64 {
    /// The next 64 random bits.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..bound` (`bound` is not zero). The remainder's bias is below
    /// `bound / 2^64`, far too small to see.
    fn below(&mut self, bound: usize) -> usize {
        // Both casts are lossless: the result is below `bound`, a `usize`.
        (self.next() % bound as u64) as usize
    }
}

/// Fisher-Yates: walk from the end, swapping each item with a random one at or before it.
/// The same seed always gives the same order.
fn shuffle_items(items: &mut [u32], seed: u32) {
    let mut random = SplitMix64(u64::from(seed));
    for i in (1..items.len()).rev() {
        items.swap(i, random.below(i + 1));
    }
}

/// A flag per item number, set for each of `items`.
pub(super) fn marks(size: usize, items: &[u32]) -> Vec<bool> {
    let mut marks = vec![false; size];
    for &item in items {
        if let Some(mark) = usize::try_from(item).ok().and_then(|at| marks.get_mut(at)) {
            *mark = true;
        }
    }
    marks
}

/// Whether an item's flag is set.
pub(super) fn marked(marks: &[bool], item: u32) -> bool {
    usize::try_from(item).ok().and_then(|at| marks.get(at)).copied().unwrap_or(false)
}

impl Queue {
    /// Shuffle the context's Up Next with `seed`, and remember the order it had. The Manual
    /// Queue, passed items and the current item are untouched. Does nothing if already
    /// shuffled, so two devices turning shuffle on at once end up with the first one's order.
    pub(super) fn shuffle(&mut self, seed: u32) {
        if self.shuffle.is_some() {
            return;
        }
        let unshuffled = self.order.clone();
        if let Some(tail) = self.order.get_mut(self.cursor..) {
            shuffle_items(tail, seed);
        }
        self.shuffle = Some(Shuffled { seed, unshuffled });
    }

    /// Put the context back in its pre-shuffle order, continuing from the current item's place
    /// in it. Does nothing if not shuffled.
    ///
    /// - Up Next is the pre-shuffle order after the current item, without anything already
    ///   passed or played. A current item from the Manual Queue counts as the last context item
    ///   started. Items before that place are passed, as when a queue starts mid-album.
    /// - Removals stick, and moves within the context revert.
    /// - Items added to the context while shuffled keep their distance from the current item.
    /// - Passed items that sit after the current item's place stay passed, ahead of the rest, so
    ///   Repeat All still replays them (after the others, in the order they were passed).
    pub(super) fn unshuffle(&mut self) {
        let Some(Shuffled { unshuffled, .. }) = self.shuffle.take() else {
            return;
        };
        let size = self.item_count();
        let present = marks(size, &self.order);
        let natural = marks(size, &unshuffled);
        let passed = marks(size, self.passed());

        let pivot = self
            .current
            .filter(|&item| marked(&present, item) && marked(&natural, item))
            .or_else(|| self.passed().iter().rev().copied().find(|&item| marked(&natural, item)));
        let kept: Vec<u32> =
            unshuffled.into_iter().filter(|&item| marked(&present, item)).collect();
        let split = pivot
            .and_then(|pivot| kept.iter().position(|&item| item == pivot))
            .map_or(0, |at| at + 1);
        let (head, rest) = kept.split_at(split);

        let mut base = rest.iter().copied().filter(|&item| !marked(&passed, item));
        let mut added = self
            .tail()
            .iter()
            .enumerate()
            .filter(|&(_, &item)| !marked(&natural, item))
            .map(|(distance, &item)| (distance, item))
            .peekable();
        let mut tail = Vec::new();
        loop {
            let item = match added.next_if(|&(distance, _)| distance <= tail.len()) {
                Some((_, item)) => item,
                None => match base.next().or_else(|| added.next().map(|(_, item)| item)) {
                    Some(item) => item,
                    None => break,
                },
            };
            tail.push(item);
        }

        let in_head = marks(size, head);
        let played: Vec<u32> =
            self.passed().iter().copied().filter(|&item| !marked(&in_head, item)).collect();
        self.cursor = played.len() + head.len();
        self.order = [&played, head, &tail].concat();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SplitMix64 from seed 1234567: the reference outputs published with the algorithm.
    #[test]
    fn shuffle_golden_generator_matches_the_reference() {
        let mut random = SplitMix64(1_234_567);
        let outputs = [0; 5].map(|_| random.next());
        assert_eq!(
            outputs,
            [
                6_457_827_717_110_365_317,
                3_203_168_211_198_807_973,
                9_817_491_932_198_370_423,
                4_593_380_528_125_082_431,
                16_408_922_859_458_223_821,
            ]
        );
    }

    /// Golden shuffle, for the wasm parity spec to repeat: the numbers `0..10` shuffled with
    /// seed 42 (Fisher-Yates from the end, swapping with `next() % (i + 1)`) come out in this
    /// order. Computed independently in Python, not from this code.
    #[test]
    fn shuffle_golden_order_is_pinned() {
        let mut items: Vec<u32> = (0..10).collect();
        shuffle_items(&mut items, 42);
        assert_eq!(items, [0, 9, 5, 8, 6, 4, 7, 2, 1, 3]);

        let mut items: Vec<u32> = (0..10).collect();
        shuffle_items(&mut items, u32::MAX);
        assert_eq!(items, [5, 9, 4, 7, 8, 2, 1, 3, 6, 0]);
    }
}
