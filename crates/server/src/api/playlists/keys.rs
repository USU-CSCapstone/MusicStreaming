//! Sparse keys for an edited playlist (`design/database.md` §6).
//!
//! After an edit the items are in their new order, each old one still carrying the key it is
//! stored under. The ones that are already in order keep it, so the edit writes only the rest:
//! the items it added, and the ones it moved.

use super::KEY_GAP;

/// The key each item is stored under once the edit is written, given its stored key, or `None`
/// if it is new, in the new order. Returns `None` when a gap between neighbours is used up (or
/// a key would overflow), and the whole playlist is to be renumbered instead.
///
/// The items that keep their key are a longest run of stored keys already increasing, so as few
/// as can be are written. Every other item takes a key between the kept items on each side of
/// it: evenly spread, so a single item takes the midpoint, and [`KEY_GAP`] apart past either end
/// of the playlist.
pub fn assign(stored: &[Option<i64>]) -> Option<Vec<i64>> {
    let kept = longest_increasing(stored);
    let mut keys = Vec::with_capacity(stored.len());
    let mut low: Option<i64> = None;
    let mut at = 0;
    while at < stored.len() {
        if let Some(key) = kept[at] {
            keys.push(key);
            low = Some(key);
            at += 1;
            continue;
        }
        let start = at;
        while at < stored.len() && kept[at].is_none() {
            at += 1;
        }
        let high = kept.get(at).copied().flatten();
        spread(low, high, at - start, &mut keys)?;
    }
    Some(keys)
}

/// Appends `count` increasing keys that lie between `low` and `high`, either of which may be
/// absent. `None` if there is no room for them.
fn spread(low: Option<i64>, high: Option<i64>, count: usize, keys: &mut Vec<i64>) -> Option<()> {
    let count = i64::try_from(count).ok()?;
    for n in 1..=count {
        keys.push(match (low, high) {
            (Some(low), Some(high)) => {
                let step = high.checked_sub(low)? / (count + 1);
                if step < 1 {
                    return None;
                }
                low + step * n
            }
            (Some(low), None) => low.checked_add(KEY_GAP.checked_mul(n)?)?,
            (None, Some(high)) => high.checked_sub(KEY_GAP.checked_mul(count + 1 - n)?)?,
            (None, None) => KEY_GAP.checked_mul(n)?,
        });
    }
    Some(())
}

/// `stored` with every key outside one longest strictly increasing run replaced by `None`.
fn longest_increasing(stored: &[Option<i64>]) -> Vec<Option<i64>> {
    let present: Vec<(usize, i64)> =
        stored.iter().enumerate().filter_map(|(at, key)| Some((at, (*key)?))).collect();
    // `tails[n]` is the index in `present` of the smallest last key of an increasing run of
    // n + 1 keys; `before` links each key to the one before it in the run that ends with it.
    let mut tails: Vec<usize> = Vec::new();
    let mut before: Vec<Option<usize>> = vec![None; present.len()];
    for (i, &(_, key)) in present.iter().enumerate() {
        let n = tails.partition_point(|&t| present[t].1 < key);
        before[i] = n.checked_sub(1).map(|n| tails[n]);
        if n == tails.len() {
            tails.push(i);
        } else {
            tails[n] = i;
        }
    }
    let mut kept = vec![None; stored.len()];
    let mut run = tails.last().copied();
    while let Some(i) = run {
        let (at, key) = present[i];
        kept[at] = Some(key);
        run = before[i];
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_already_in_order_keep_their_keys() {
        assert_eq!(assign(&[Some(5), Some(9), Some(40)]), Some(vec![5, 9, 40]));
        assert_eq!(assign(&[]), Some(vec![]));
    }

    #[test]
    fn a_moved_item_is_the_only_one_to_change() {
        let keys = assign(&[Some(20), Some(30), Some(40), Some(50), Some(10)]).unwrap();
        assert_eq!(keys, [20, 30, 40, 50, 10].map(|k| if k == 10 { 50 + KEY_GAP } else { k }));
        // The block moved to the front takes keys before the first kept one, in order.
        let keys = assign(&[Some(40), Some(50), Some(10), Some(20), Some(30), Some(60)]).unwrap();
        assert_eq!(keys, [10 - 2 * KEY_GAP, 10 - KEY_GAP, 10, 20, 30, 60]);
    }

    #[test]
    fn new_items_take_the_midpoint_or_an_even_spread_between_neighbours() {
        assert_eq!(assign(&[Some(0), None, Some(100)]), Some(vec![0, 50, 100]));
        assert_eq!(assign(&[Some(0), None, None, Some(9)]), Some(vec![0, 3, 6, 9]));
        assert_eq!(assign(&[None, None]), Some(vec![KEY_GAP, 2 * KEY_GAP]));
        assert_eq!(assign(&[Some(7), None]), Some(vec![7, 7 + KEY_GAP]));
    }

    #[test]
    fn a_gap_used_up_asks_for_a_renumber() {
        assert_eq!(assign(&[Some(1), None, Some(2)]), None);
        assert_eq!(assign(&[Some(1), None, None, Some(3)]), None);
        assert_eq!(assign(&[Some(1), None, None, Some(4)]), Some(vec![1, 2, 3, 4]));
        assert_eq!(assign(&[Some(i64::MAX), None]), None);
        assert_eq!(assign(&[None, Some(i64::MIN + 1)]), None);
    }

    /// For every order of six keys, the keys come out strictly increasing, and as many items
    /// keep theirs as the longest increasing run has.
    #[test]
    fn the_kept_items_are_a_longest_increasing_run() {
        fn permutations(items: &mut Vec<i64>, from: usize, all: &mut Vec<Vec<i64>>) {
            if from == items.len() {
                all.push(items.clone());
            }
            for i in from..items.len() {
                items.swap(from, i);
                permutations(items, from + 1, all);
                items.swap(from, i);
            }
        }
        let mut all = Vec::new();
        permutations(&mut (1..=6).map(|k| k * 10).collect(), 0, &mut all);
        for order in all {
            let longest = (0..1u32 << 6)
                .filter(|mask| {
                    let run: Vec<i64> =
                        (0..6).filter(|i| mask >> i & 1 == 1).map(|i| order[i]).collect();
                    run.windows(2).all(|pair| pair[0] < pair[1])
                })
                .map(u32::count_ones)
                .max()
                .unwrap();
            let stored: Vec<Option<i64>> = order.iter().copied().map(Some).collect();
            let keys = assign(&stored).unwrap();
            assert!(keys.windows(2).all(|pair| pair[0] < pair[1]), "{order:?} {keys:?}");
            let kept = keys.iter().zip(&order).filter(|(new, old)| new == old).count();
            assert_eq!(kept, longest as usize, "{order:?}");
        }
    }
}
