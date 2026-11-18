use super::tests::doc;
use super::*;

/// Build and query times at the full-scale target: 500,000 tracks, 50,000 albums, and
/// 20,000 artists named from a Zipf-distributed vocabulary. Run with
/// `cargo test --release -p jewelcase-core benchmark_full_scale -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn benchmark_full_scale() {
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
        let i = usize::try_from(((vocabulary.len() as f64).powf(r) - 1.0) as i64).unwrap();
        vocabulary[i.min(vocabulary.len() - 1)].clone()
    };
    let mut documents = Vec::new();
    for (kind, count, words) in
        [(Kind::Track, 500_000, 4), (Kind::Album, 50_000, 3), (Kind::Artist, 20_000, 2)]
    {
        for id in 0..count {
            let n = 1 + usize::try_from(id % words).unwrap();
            let name = (0..n).map(|_| word()).collect::<Vec<_>>().join(" ");
            documents.push(doc(kind, id, &name));
        }
    }
    let started = std::time::Instant::now();
    let index = Index::build(documents);
    println!(
        "built {} documents, {} words, in {:?}",
        index.ids.len(),
        index.vocabulary_len(),
        started.elapsed()
    );

    let common = (0..index.vocabulary_len())
        .max_by_key(|&w| index.posting(w).len())
        .map(|w| index.word(w).to_owned())
        .unwrap();
    let mut queries = vec!["a".to_owned(), "s".to_owned(), "ab".to_owned(), common.clone()];
    let long = (0..index.vocabulary_len())
        .map(|w| index.word(w))
        .find(|w| w.len() >= 8)
        .unwrap()
        .to_owned();
    let mut typo = long.clone();
    typo.replace_range(3..4, "z");
    queries.extend([typo, format!("{common} {long}"), format!("{} {}", &long[..3], &common[..2])]);
    // Queries led by an optional article.
    queries.extend([format!("a {common}"), format!("a {long}")]);
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

    // Near misses of queries that find nothing (the vocabulary has no `z`): a wrong first
    // letter, a typo more than a search allows, a word matching nothing beside the commonest,
    // which every one of its names nearly matches, and nothing near at all. Each matches the
    // whole vocabulary again, whatever its first letter.
    let mut wrong_first = long.clone();
    wrong_first.replace_range(0..1, "z");
    let mut three_typos = long.clone();
    for at in [1, 3, 5] {
        three_typos.replace_range(at..at + 1, "z");
    }
    let near = [
        wrong_first,
        three_typos,
        format!("{common} zzzzzz"),
        format!("{long} {common} zzzzzz"),
        "zzzzzzzz".to_owned(),
    ];
    for query in &near {
        assert!(index.search(query, &Kind::ALL, 5).is_empty(), "{query} found something");
        let started = std::time::Instant::now();
        let rounds = 10;
        let mut misses = Vec::new();
        for _ in 0..rounds {
            misses = index.near_misses(query, &Kind::ALL, 3, |_, _| true);
        }
        println!(
            "{query:>22}: {:>8.2?} per near miss, {} found",
            started.elapsed() / rounds,
            misses.len()
        );
    }

    // A page of tracks deep in the commonest word's, continuing after a rank, which compares
    // names wherever scores tie.
    for query in ["a", common.as_str()] {
        let first = index.page(query, Kind::Track, None, None, 1000, |_, _| true);
        let started = std::time::Instant::now();
        let rounds = 20;
        for _ in 0..rounds {
            index.page(query, Kind::Track, None, first.next.as_ref(), 100, |_, _| true);
        }
        println!(
            "{query:>22}: {:>8.2?} per page after 1,000 of {}",
            started.elapsed() / rounds,
            first.total
        );
    }
}

/// The lyric index's size and build time at the heavy case its budget is set for: 150,000 tracks
/// with lyrics (`design/search.md` §2.5), each 30 lines of its own and a chorus of five sung three
/// times, so 45 lines and 35 distinct, of 4 to 10 words from a 60,000-word vocabulary whose
/// words occur as a language's do (Zipf-like, the commonest far the most). Run with
/// `cargo test --release -p jewelcase-core benchmark_lyrics_full_scale -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn benchmark_lyrics_full_scale() {
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let letters: Vec<char> = "abcdefghijklmnoprstuvwy".chars().collect();
    let vocabulary: Vec<String> = (0..60_000)
        .map(|_| {
            let len = 1 + (next() % 9) as usize;
            (0..len).map(|_| letters[(next() % letters.len() as u64) as usize]).collect()
        })
        .collect();
    let mut line = || {
        let words = 4 + (next() % 7) as usize;
        let words = (0..words).map(|_| {
            let r = (next() % 1_000_000) as f64 / 1_000_000.0;
            let i = usize::try_from(((vocabulary.len() as f64).powf(r) - 1.0) as i64).unwrap();
            vocabulary[i.min(vocabulary.len() - 1)].as_str()
        });
        words.collect::<Vec<_>>().join(" ")
    };
    let tracks: Vec<(i64, Vec<String>)> = (0..150_000)
        .map(|id| {
            let chorus: Vec<String> = (0..5).map(|_| line()).collect();
            let mut lines = Vec::with_capacity(45);
            for _verse in 0..3 {
                lines.extend((0..10).map(|_| line()));
                lines.extend(chorus.iter().cloned());
            }
            (id, lines)
        })
        .collect();
    let given: usize = tracks.iter().map(|(_, lines)| lines.len()).sum();

    let started = std::time::Instant::now();
    let index = LyricIndex::build(tracks);
    let took = started.elapsed();
    println!(
        "{} tracks, {given} lines given, {} indexed, built in {took:?}: {:.1} MB",
        index.tracks(),
        index.lines(),
        index.bytes() as f64 / 1_000_000.0
    );
    for (part, bytes) in index.parts() {
        println!("{part:>24}: {:>6.1} MB", bytes as f64 / 1_000_000.0);
    }

    // Searches: a first letter, as typing starts; the commonest word, which most lines hold; two
    // common words together; a rare word; a common word and a rare one with a typo; and a phrase
    // of four.
    let common = &vocabulary[0];
    let second = &vocabulary[1];
    let rare = vocabulary.iter().rev().find(|w| w.len() >= 6).unwrap();
    let mut typo = rare.clone();
    typo.replace_range(3..4, "z");
    let queries = [
        common[..1].to_owned(),
        common.clone(),
        format!("{common} {second}"),
        rare.clone(),
        format!("{common} {typo}"),
        format!("{common} {second} {} {}", vocabulary[2], vocabulary[3]),
    ];
    for query in &queries {
        let started = std::time::Instant::now();
        let rounds = 10;
        let mut found = 0;
        for _ in 0..rounds {
            found = index.search(query, |_| true).len();
        }
        println!("{query:>28}: {:>8.2?} per search, {found} tracks", started.elapsed() / rounds);
    }
}
