//! The shuffle permutation: a seeded swap-or-not shuffle (Hoang, Morris and Rogaway, 2012),
//! computed one position at a time (`design/queue.md` §4).
//!
//! `(seed, m, R)` fix the permutation, and saved queues and the golden vectors in the tests
//! depend on it, so nothing below may change. Round `i` (0-based) of the `R` rounds uses
//! two outputs of the SplitMix64 generator seeded with `seed as u64`: output `2i` is
//! reduced `% m` to the pivot `K`, and output `2i + 1` is the round's coin key `C`. A value
//! `x` pairs with `x' = (K - x) mod m`, and the pair swaps when the top bit of
//! `mix(C ^ max(x, x'))` is 1, where `mix` is the SplitMix64 finalizer. Both members of a
//! pair see the same coin, so a round is its own inverse. R = 6⌈log₂ m⌉ + 6.

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "nothing reads the permutation until the shuffle step")
)]

/// The SplitMix64 increment.
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// The SplitMix64 output finalizer: a bijection on `u64`.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The `n`th (0-based) output of SplitMix64 started from state `seed`.
fn splitmix(seed: u32, n: u64) -> u64 {
    mix(u64::from(seed).wrapping_add(n.wrapping_add(1).wrapping_mul(GAMMA)))
}

/// The number of rounds for a list of `m` rows.
fn rounds(m: u64) -> u32 {
    6 * (u64::BITS - m.saturating_sub(1).leading_zeros()) + 6
}

/// Round `i` applied to `x`: returns its partner if the round's coin says to swap, else `x`.
/// Its own inverse.
fn round(seed: u32, m: u64, i: u32, x: u64) -> u64 {
    let pivot = splitmix(seed, 2 * u64::from(i)) % m;
    let partner = if pivot >= x { pivot - x } else { pivot + (m - x) };
    let high = x.max(partner);
    if mix(splitmix(seed, 2 * u64::from(i) + 1) ^ high) >> 63 == 1 { partner } else { x }
}

/// Where position `x` of the unshuffled list lands, for a list of `m` rows. `x` must be
/// below `m`. O(log m) time and O(1) memory.
pub(super) fn permute(seed: u32, m: u64, x: u64) -> u64 {
    permute_with(seed, m, rounds(m), x)
}

/// The position that lands at `y`: the inverse of [`permute`]. `y` must be below `m`.
pub(super) fn unpermute(seed: u32, m: u64, y: u64) -> u64 {
    unpermute_with(seed, m, rounds(m), y)
}

/// [`permute`] with an explicit round count (the experiment varies it).
fn permute_with(seed: u32, m: u64, rounds: u32, mut x: u64) -> u64 {
    debug_assert!(x < m);
    if m < 2 {
        return x;
    }
    for i in 0..rounds {
        x = round(seed, m, i, x);
    }
    x
}

/// [`unpermute`] with an explicit round count.
fn unpermute_with(seed: u32, m: u64, rounds: u32, mut y: u64) -> u64 {
    debug_assert!(y < m);
    if m < 2 {
        return y;
    }
    for i in (0..rounds).rev() {
        y = round(seed, m, i, y);
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEEDS: [u32; 4] = [0, 1, 7, u32::MAX];

    #[test]
    fn permutation_is_bijection() {
        for m in (1..=70).chain([100, 255, 256, 257, 1000, 4097]) {
            for seed in SEEDS {
                let mut seen = vec![false; m as usize];
                for x in 0..m {
                    let y = permute(seed, m, x);
                    assert!(y < m);
                    assert!(!seen[y as usize], "m {m} seed {seed}: {y} twice");
                    seen[y as usize] = true;
                }
            }
        }
    }

    #[test]
    fn permutation_round_trips() {
        for m in [1, 2, 3, 10, 12, 500_000, 1 << 32, (1 << 40) + 3, u64::MAX - 1, u64::MAX] {
            for seed in SEEDS {
                for k in 0..200u64 {
                    let x = (k.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ k) % m;
                    let y = permute(seed, m, x);
                    assert!(y < m);
                    assert_eq!(unpermute(seed, m, y), x, "m {m} seed {seed} x {x}");
                    assert_eq!(permute(seed, m, unpermute(seed, m, x)), x);
                }
                for x in [0, m - 1] {
                    assert_eq!(unpermute(seed, m, permute(seed, m, x)), x);
                }
            }
        }
    }

    #[test]
    fn permutation_tiny_sizes() {
        assert_eq!(permute(5, 1, 0), 0);
        assert_eq!(unpermute(5, 1, 0), 0);
        for seed in 0..64 {
            let a = [permute(seed, 2, 0), permute(seed, 2, 1)];
            assert!(a == [0, 1] || a == [1, 0], "seed {seed}: {a:?}");
            assert_eq!(unpermute(seed, 2, a[0]), 0);
        }
        let swapped = (0..64).filter(|&s| permute(s, 2, 0) == 1).count();
        assert!((10..54).contains(&swapped), "m = 2 swapped for {swapped} of 64 seeds");
    }

    #[test]
    fn permutation_golden_vectors() {
        let table: [(u32, u64, u64, u64); 18] = [
            (0, 1, 0, 0),
            (0, 8, 3, 6),
            (1, 16, 5, 10),
            (1, 2, 0, 0),
            (1, 2, 1, 1),
            (0, 3, 0, 1),
            (0, 3, 2, 0),
            (7, 3, 1, 1),
            (0, 10, 0, 3),
            (0, 10, 9, 2),
            (42, 10, 4, 8),
            (1, 12, 5, 6),
            (4294967295, 12, 11, 10),
            (0, 500000, 0, 168416),
            (42, 500000, 123456, 491515),
            (4294967295, 500000, 499999, 446286),
            (3, 4294967301, 4294967300, 2336714214),
            (9, 1099511627779, 12345, 343340338295),
        ];
        for (seed, m, x, y) in table {
            assert_eq!(permute(seed, m, x), y, "seed {seed} m {m} x {x}");
        }
    }

    /// Chi-square per degree of freedom of counts against a uniform expectation.
    fn chi(counts: &[u32], total: f64) -> f64 {
        let e = total / counts.len() as f64;
        let sum: f64 = counts.iter().map(|&c| (f64::from(c) - e).powi(2) / e).sum();
        sum / (counts.len() - 1) as f64
    }

    /// Evenness of the permutation at several round counts. Run:
    /// `cargo test -p jewelcase-core --release --locked permutation_evenness -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn permutation_evenness() {
        // Chance that L given values land as one block of consecutive positions, for a
        // uniform permutation: (m - L + 1) / C(m, L).
        let block = |m: u64, l: u64| {
            let mut c = 1.0f64;
            for k in 0..l {
                c *= (m - k) as f64 / (k + 1) as f64;
            }
            (m - l + 1) as f64 / c
        };
        println!(
            "m, rounds, seeds | coverage chi2/dof (1 = uniform) | adjacent x(2/m) | run3 x | run5 x"
        );
        for (m, seeds) in [
            (10u64, 20_000u32),
            (12, 20_000),
            (100, 4000),
            (1000, 600),
            (10_000, 100),
            (60_000, 12),
        ] {
            let log = u64::BITS - (m - 1).leading_zeros();
            let mut variants: Vec<u32> = (1..=8).map(|c| c * log).collect();
            variants.push(rounds(m));
            variants.push(rounds(m) * 2);
            for r in variants {
                let probes = [0, 1, m / 2, m - 1];
                let mut worst = 0.0f64;
                for &x in &probes {
                    let mut counts = vec![0u32; m as usize];
                    for s in 0..seeds * 4 {
                        counts[permute_with(s, m, r, x) as usize] += 1;
                    }
                    worst = worst.max(chi(&counts, f64::from(seeds * 4)));
                }
                let (mut adj, mut r3, mut r5, mut n2, mut n3, mut n5) =
                    (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
                for s in 0..seeds {
                    let img: Vec<u64> = (0..m).map(|x| permute_with(s, m, r, x)).collect();
                    for w in img.windows(2) {
                        n2 += 1;
                        adj += u64::from(w[0].abs_diff(w[1]) == 1);
                    }
                    for (l, hit, n) in [(3usize, &mut r3, &mut n3), (5, &mut r5, &mut n5)] {
                        if (m as usize) < l {
                            continue;
                        }
                        for w in img.windows(l) {
                            *n += 1;
                            let (lo, hi) = (w.iter().min().unwrap(), w.iter().max().unwrap());
                            *hit += u64::from(hi - lo == l as u64 - 1);
                        }
                    }
                }
                let ratio = |hit: u64, n: u64, p: f64| {
                    if n == 0 { f64::NAN } else { hit as f64 / n as f64 / p }
                };
                // A uniform permutation keeps a pair adjacent with probability 2/m.
                let tag = if r == rounds(m) { " <- formula" } else { "" };
                println!(
                    "{m:6}, {r:3}, {seeds:6} | {worst:6.2} | {:6.2} | {:6.2} | {:6.2}{tag}",
                    ratio(adj, n2, 2.0 / m as f64),
                    ratio(r3, n3, block(m, 3)),
                    ratio(r5, n5, block(m, 5)),
                );
            }
        }
    }

    /// Time for a 50-row window at m = 500,000. Run in release with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn permutation_window_timing() {
        let m = 500_000;
        let t = std::time::Instant::now();
        let mut sum = 0u64;
        let reps = 20_000u64;
        for s in 0..reps {
            for x in 0..50 {
                sum = sum.wrapping_add(permute(s as u32, m, 1000 + x));
            }
        }
        println!(
            "50-row window at m=500000: {:?} per window (sum {sum})",
            t.elapsed() / reps as u32
        );
    }
}
