use super::tests::doc;
use super::*;

/// Build and query times at the full-scale target: 500,000 tracks, 50,000 albums, and
/// 20,000 artists named from a Zipf-distributed vocabulary. Run with
/// `cargo test --release -p jewelcase-core search::tests::full_scale -- --ignored --nocapture`.
#[test]
#[ignore]
fn full_scale() {
    // A deterministic generator, so every run measures the same library.
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let letters: Vec<char> = "abcdefghijklmnoprstuvwy".chars().collect();
    let vocabulary: Vec<String> = (0..40_000)
        .map(|_| {
            let len = 2 + (next() % 8) as usize;
            (0..len).map(|_| letters[(next() % letters.len() as u64) as usize]).collect()
        })
        .collect();
    let mut word = || {
        // Zipf-like: small indexes are far more common.
        let r = (next() % 1_000_000) as f64 / 1_000_000.0;
        let i = ((vocabulary.len() as f64).powf(r) - 1.0) as usize;
        vocabulary[i.min(vocabulary.len() - 1)].clone()
    };
    let mut documents = Vec::new();
    for (kind, count, words) in
        [(Kind::Track, 500_000, 4), (Kind::Album, 50_000, 3), (Kind::Artist, 20_000, 2)]
    {
        for id in 0..count {
            let n = 1 + (id % words) as usize;
            let name = (0..n).map(|_| word()).collect::<Vec<_>>().join(" ");
            documents.push(doc(kind, id, &name));
        }
    }
    let started = std::time::Instant::now();
    let index = Index::build(documents);
    println!(
        "built {} documents, {} words, in {:?}",
        index.len(),
        index.vocabulary.len(),
        started.elapsed()
    );

    let common = index
        .vocabulary
        .iter()
        .max_by_key(|w| index.postings[index.prefixed(w).start].len())
        .unwrap()
        .clone();
    let mut queries = vec!["a".to_owned(), "s".to_owned(), "ab".to_owned(), common.clone()];
    let long = index.vocabulary.iter().find(|w| w.len() >= 8).unwrap().clone();
    let mut typo = long.clone();
    typo.replace_range(3..4, "z");
    queries.extend([typo, format!("{common} {long}"), format!("{} {}", &long[..3], &common[..2])]);
    for query in &queries {
        let started = std::time::Instant::now();
        let rounds = 20;
        let mut sections = Vec::new();
        for _ in 0..rounds {
            sections = index.search(query, &Kind::ALL, 5);
        }
        let totals: Vec<usize> = sections.iter().map(|s| s.total).collect();
        println!("{query:>22}: {:>8.2?} per search, totals {totals:?}", started.elapsed() / rounds);
    }
}
