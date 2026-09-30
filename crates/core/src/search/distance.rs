//! Edit distance with transpositions, for typo tolerance.

/// The typos between `query` and `word`, counting a transposed pair as one, and whether that
/// is to a prefix of `word` rather than all of it. `None` if more than `max`.
pub fn distance(query: &[char], word: &[char], max: u8) -> Option<(u8, bool)> {
    let m = query.len();
    let max = usize::from(max);
    // One column per character of `word`: column j holds the distance from each prefix of
    // `query` to `word[..j]`. The two before it are kept, for transpositions.
    let mut before: Vec<usize> = vec![0; m + 1];
    let mut previous: Vec<usize> = (0..=m).collect();
    let mut current: Vec<usize> = vec![0; m + 1];
    let mut to_prefix = m;
    let mut to_whole = if word.is_empty() { m } else { usize::MAX };
    for j in 1..=word.len() {
        current[0] = j;
        for i in 1..=m {
            let cost = usize::from(query[i - 1] != word[j - 1]);
            let mut d = (previous[i] + 1).min(current[i - 1] + 1).min(previous[i - 1] + cost);
            if i > 1 && j > 1 && query[i - 1] == word[j - 2] && query[i - 2] == word[j - 1] {
                d = d.min(before[i - 2] + 1);
            }
            current[i] = d;
        }
        to_prefix = to_prefix.min(current[m]);
        if j == word.len() {
            to_whole = current[m];
        }
        // A column's distances never shrink by more than a transposition can undo, so once
        // two columns in a row are past `max`, nothing further can come back within it.
        let past = |column: &[usize]| column.iter().all(|&d| d > max);
        if past(&current) && past(&previous) {
            break;
        }
        std::mem::swap(&mut before, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    if to_whole <= max && to_whole <= to_prefix {
        Some((to_whole as u8, false))
    } else if to_prefix <= max {
        Some((to_prefix as u8, true))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_counts_typos_to_the_word_or_a_prefix() {
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        let d = |a: &str, b: &str, max| distance(&chars(a), &chars(b), max);
        assert_eq!(d("swift", "swift", 1), Some((0, false)));
        assert_eq!(d("swi", "swift", 1), Some((0, true)));
        assert_eq!(d("sweft", "swift", 1), Some((1, false)));
        assert_eq!(d("tyalor", "taylor", 1), Some((1, false)), "a transposition is one typo");
        assert_eq!(d("swfi", "swiftly", 1), Some((1, true)));
        assert_eq!(d("swift", "sw", 1), None);
        assert_eq!(d("abcdef", "uvwxyz", 2), None);
    }
}
