use super::*;

/// A document of `kind` named `name`, made directly rather than through a constructor.
pub(super) fn doc(kind: Kind, id: i64, name: &str) -> Document {
    Document { kind, group: None, id, name: name.to_owned(), weight: 0 }
}

/// An index of a small library: a few artists, albums, and tracks with names that share words.
fn library() -> Index {
    Index::build([
        doc(Kind::Artist, 1, "Taylor Swift"),
        doc(Kind::Artist, 2, "Björk"),
        doc(Kind::Artist, 3, "The Beatles"),
        doc(Kind::Artist, 4, "Weezer"),
        doc(Kind::Album, 10, "Weezer (Blue Album)"),
        doc(Kind::Album, 11, "Blue"),
        doc(Kind::Track, 20, "Say It Ain't So"),
        doc(Kind::Track, 21, "Blue"),
        doc(Kind::Track, 22, "Beatles Medley"),
        doc(Kind::Track, 23, "Hello"),
        doc(Kind::Track, 24, "Hello World"),
        doc(Kind::Track, 25, "Hallo"),
        doc(Kind::Track, 26, "Swiftly"),
    ])
}

/// Each section's kind and IDs, in order.
fn find(index: &Index, query: &str) -> Vec<(Kind, Vec<i64>)> {
    index
        .search(query, &Kind::ALL, 10)
        .into_iter()
        .map(|section| (section.kind, section.ids))
        .collect()
}

/// The single best match.
fn top(index: &Index, query: &str) -> Option<i64> {
    index.search(query, &Kind::ALL, 1).first().map(|section| section.ids[0])
}

#[test]
fn tracks_titled_albums_and_named_artists_are_searched() {
    assert_eq!(Document::from_track(1, "Blue".into()), doc(Kind::Track, 1, "Blue"));
    assert_eq!(Document::from_album(2, Some("Blue".into())), Some(doc(Kind::Album, 2, "Blue")));
    assert_eq!(
        Document::from_artist(3, Some("Weezer".into())),
        Some(doc(Kind::Artist, 3, "Weezer"))
    );
    assert_eq!(Document::from_album(4, None), None);
    assert_eq!(Document::from_artist(5, None), None);
}

#[test]
fn typos_do_not_get_in_the_way() {
    let index = library();
    assert_eq!(top(&index, "tylor sweft"), Some(1), "missing and wrong letters");
    assert_eq!(top(&index, "taylro swift"), Some(1), "transposed letters");
    assert_eq!(top(&index, "taylorr swift"), Some(1), "doubled letters");
}

#[test]
fn accents_case_and_word_order_are_irrelevant() {
    let index = library();
    assert_eq!(top(&index, "bjork"), Some(2));
    assert_eq!(top(&index, "BJÖRK"), Some(2));
    assert_eq!(top(&index, "swift taylor"), Some(1));
}

#[test]
fn partial_input_matches() {
    let index = library();
    assert_eq!(top(&index, "tay"), Some(1));
    assert_eq!(top(&index, "say it ain"), Some(20));
    assert_eq!(top(&index, "say it aint so"), Some(20));
}

#[test]
fn leading_articles_do_not_matter() {
    let index = library();
    // "beatles" is The Beatles exactly, above the track that merely contains the word.
    assert_eq!(find(&index, "beatles"), [(Kind::Artist, vec![3]), (Kind::Track, vec![22])]);
    assert_eq!(top(&index, "the beatles"), Some(3));
}

#[test]
fn a_leading_article_in_the_query_is_optional() {
    let index = Index::build([
        doc(Kind::Artist, 1, "Beatles"),
        doc(Kind::Artist, 2, "A Perfect Circle"),
        doc(Kind::Artist, 3, "An Horse"),
        doc(Kind::Track, 4, "Beatles Medley"),
        doc(Kind::Track, 5, "Perfect Circle Live"),
        doc(Kind::Track, 6, "A Perfect Circle Live"),
        doc(Kind::Track, 7, "Horse"),
        doc(Kind::Album, 8, "Beatles, The"),
    ]);
    // A name without the article matches it exactly, above the same words in another order.
    assert_eq!(
        find(&index, "the beatles"),
        [(Kind::Artist, vec![1]), (Kind::Album, vec![8]), (Kind::Track, vec![4])]
    );
    // Of names equally exact, the one with the article comes first.
    assert_eq!(find(&index, "an horse"), [(Kind::Artist, vec![3]), (Kind::Track, vec![7])]);
    assert_eq!(
        find(&index, "a perfect circle"),
        [(Kind::Artist, vec![2]), (Kind::Track, vec![6, 5])]
    );
    // A skipped article counts as no matched word, so "Beatles Medley" has a word beyond the
    // query's.
    assert_eq!(
        find(&index, "the beat"),
        [(Kind::Album, vec![8]), (Kind::Artist, vec![1]), (Kind::Track, vec![4])]
    );
    // A query that is only an article must match it.
    assert_eq!(find(&index, "an"), [(Kind::Artist, vec![3])]);
}

#[test]
fn a_name_that_matches_the_article_ranks_first() {
    let index = Index::build([
        doc(Kind::Artist, 1, "The Killers"),
        doc(Kind::Track, 2, "Killers"),
        doc(Kind::Track, 3, "The Beast"),
        doc(Kind::Track, 4, "Baby"),
    ]);
    // Kind and name order alone would put each the other way round.
    assert_eq!(find(&index, "the killers"), [(Kind::Artist, vec![1]), (Kind::Track, vec![2])]);
    assert_eq!(find(&index, "the b"), [(Kind::Track, vec![3, 4])]);
}

#[test]
fn a_skipped_article_costs_less_than_a_typo_and_more_than_an_extra_word() {
    let index = Index::build([
        doc(Kind::Track, 1, "Beatles Medley"),
        doc(Kind::Track, 2, "The Beetles Tribute"),
        doc(Kind::Track, 3, "The Beast Within"),
        doc(Kind::Track, 4, "Beautiful"),
    ]);
    assert_eq!(find(&index, "the beatles"), [(Kind::Track, vec![1, 2])]);
    // "Beautiful" has no word beyond the query's but skipped the article, so it follows
    // "The Beast Within".
    assert_eq!(find(&index, "the bea"), [(Kind::Track, vec![3, 4, 1])]);
}

#[test]
fn every_word_but_a_leading_article_must_match() {
    assert_eq!(find(&library(), "blue weezer"), [(Kind::Album, vec![10])]);
}

#[test]
fn a_name_that_is_only_an_article_keeps_it() {
    let index = Index::build([doc(Kind::Track, 1, "The"), doc(Kind::Artist, 2, "The The")]);
    assert_eq!(find(&index, "the"), [(Kind::Track, vec![1]), (Kind::Artist, vec![2])]);
}

#[test]
fn a_leading_article_in_the_name_is_optional() {
    let index = Index::build([
        doc(Kind::Artist, 1, "A Perfect Circle"),
        doc(Kind::Artist, 2, "An Horse"),
        doc(Kind::Track, 3, "Perfect Circle Live"),
        doc(Kind::Track, 4, "Horse Medley"),
    ]);
    assert_eq!(find(&index, "perfect circle"), [(Kind::Artist, vec![1]), (Kind::Track, vec![3])]);
    assert_eq!(find(&index, "horse"), [(Kind::Artist, vec![2]), (Kind::Track, vec![4])]);
}

#[test]
fn compatibility_forms_and_final_sigma_match_exactly() {
    let index = Index::build([
        doc(Kind::Track, 1, "\u{FB01}re"),
        doc(Kind::Artist, 2, "Νίκος"),
        doc(Kind::Track, 3, "Νίκου"),
    ]);
    assert_eq!(find(&index, "fire"), [(Kind::Track, vec![1])]);
    // The artist is exact and the track one typo away, so the artist comes first.
    assert_eq!(find(&index, "ΝΙΚΟΣ"), [(Kind::Artist, vec![2]), (Kind::Track, vec![3])]);
}

#[test]
fn exact_matches_always_win() {
    let index = library();
    // Exact, then the same word with more after it, then one typo away.
    assert_eq!(find(&index, "hello"), [(Kind::Track, vec![23, 24, 25])]);
    assert_eq!(find(&index, "hallo"), [(Kind::Track, vec![25, 23, 24])]);
    // A prefix never beats the whole word.
    assert_eq!(find(&index, "swift"), [(Kind::Artist, vec![1]), (Kind::Track, vec![26])]);
}

#[test]
fn sections_order_by_their_best_match() {
    let index = library();
    // The artist is exact; the album only contains the word.
    assert_eq!(find(&index, "weezer"), [(Kind::Artist, vec![4]), (Kind::Album, vec![10])]);
    // Equally exact: the track, then the album.
    assert_eq!(find(&index, "blue"), [(Kind::Track, vec![21]), (Kind::Album, vec![11, 10])]);
}

#[test]
fn totals_count_past_the_limit() {
    let index = library();
    let sections = index.search("h", &Kind::ALL, 1);
    assert_eq!(sections.len(), 1);
    // "Hallo" and "Hello" tie, and "hallo" sorts first.
    assert_eq!(sections[0].ids, [25]);
    assert_eq!(sections[0].total, 3);
}

#[test]
fn kinds_filter_the_sections() {
    let index = library();
    let sections = index.search("blue", &[Kind::Album], 10);
    let found: Vec<_> =
        sections.iter().map(|s| (s.kind, s.group.clone(), &s.ids, s.total)).collect();
    assert_eq!(found, [(Kind::Album, None, &vec![11, 10], 2)]);
}

#[test]
fn nothing_to_match_finds_nothing() {
    let index = library();
    assert!(index.search("", &Kind::ALL, 10).is_empty());
    assert!(index.search("?!", &Kind::ALL, 10).is_empty());
    assert!(index.search("zzzz", &Kind::ALL, 10).is_empty());
    assert!(Index::build([]).search("blue", &Kind::ALL, 10).is_empty());
}

#[test]
fn the_first_letter_must_be_right() {
    let index = library();
    assert_eq!(top(&index, "baylor"), None);
}

#[test]
fn short_words_allow_no_typos() {
    let index = Index::build([doc(Kind::Track, 1, "Cat"), doc(Kind::Track, 2, "Bat")]);
    assert_eq!(find(&index, "cat"), [(Kind::Track, vec![1])]);
}

#[test]
fn ties_break_by_name_then_id() {
    let index = Index::build([
        doc(Kind::Track, 3, "Love B"),
        doc(Kind::Track, 2, "Love A"),
        doc(Kind::Track, 1, "Love A"),
    ]);
    assert_eq!(find(&index, "love"), [(Kind::Track, vec![1, 2, 3])]);
}

#[test]
fn a_filter_narrows_the_matches_before_they_are_ranked_and_counted() {
    let index = library();
    let blue = |keep: fn(Kind, i64) -> bool| -> Vec<(Kind, Vec<i64>, usize)> {
        index
            .search_within("blue", &Kind::ALL, 1, keep)
            .into_iter()
            .map(|section| (section.kind, section.ids, section.total))
            .collect()
    };
    assert_eq!(
        blue(|_, _| true),
        [(Kind::Track, vec![21], 1), (Kind::Album, vec![11], 2)],
        "the exact album first"
    );
    assert_eq!(blue(|_, id| id != 11), [(Kind::Track, vec![21], 1), (Kind::Album, vec![10], 1)]);
    assert_eq!(blue(|kind, _| kind == Kind::Album), [(Kind::Album, vec![11], 2)]);
    assert!(blue(|_, _| false).is_empty());
}

#[test]
fn an_index_made_a_little_at_a_time_is_the_index_made_at_once() {
    let words = ["blue", "red", "the", "a", "zebra", "Ünï", "ghost", "x"];
    let documents = || {
        (0..300_usize).map(|i| {
            let name: Vec<&str> = (0..=(i * 7 % 4)).map(|j| words[(i * 3 + j * 5) % 8]).collect();
            doc(Kind::ALL[i % 3], i64::try_from(i * 37 % 301).unwrap(), &name.join(" "))
        })
    };
    let whole = format!("{:?}", Index::build(documents()));
    for budget in [1, 2, 7, 64, 1000] {
        let mut builder = IndexBuilder::default();
        documents().for_each(|document| builder.add(document));
        let mut finishing = builder.finishing();
        let mut steps = 1;
        while !finishing.step(budget) {
            steps += 1;
        }
        assert_eq!(format!("{:?}", finishing.into_index()), whole, "budget {budget}");
        assert!(steps > 300 / budget, "budget {budget} took {steps} steps");
    }
}

/// Every match of `kind` for `query`, read `limit` at a time, as the pages' IDs.
fn pages(index: &Index, query: &str, kind: Kind, limit: usize) -> Vec<Vec<i64>> {
    let mut pages = Vec::new();
    let mut after = None;
    loop {
        let page = index.page(query, kind, None, after.as_ref(), limit, |_, _| true);
        pages.push(page.ids);
        match page.next {
            Some(next) => after = Some(next),
            None => return pages,
        }
    }
}

#[test]
fn a_section_reads_a_page_at_a_time_in_its_ranking() {
    let index = library();
    // "Hallo" and "Hello" tie, and "hallo" sorts first; "Hello World" has a word more.
    assert_eq!(index.search("h", &[Kind::Track], 10)[0].ids, [25, 23, 24]);
    assert_eq!(pages(&index, "h", Kind::Track, 2), [vec![25, 23], vec![24]]);
    assert_eq!(pages(&index, "h", Kind::Track, 3), [vec![25, 23, 24]], "no page past the last");
    let first = index.page("h", Kind::Track, None, None, 2, |_, _| true);
    assert_eq!(first.total, 3);
    let rest = index.page("h", Kind::Track, None, first.next.as_ref(), 2, |_, _| true);
    assert_eq!((rest.total, rest.next), (3, None), "the total counts every match");
}

#[test]
fn pages_put_together_are_the_whole_section() {
    let index = Index::build((0..25).map(|id| doc(Kind::Track, id, &format!("Love {}", id % 7))));
    let whole = index.search("love", &[Kind::Track], 100);
    assert_eq!(pages(&index, "love", Kind::Track, 4).concat(), whole[0].ids);
}

#[test]
fn a_page_is_only_of_its_kind_and_what_the_filter_keeps() {
    let index = library();
    let blue = |keep: fn(Kind, i64) -> bool| {
        let page = index.page("blue", Kind::Album, None, None, 10, keep);
        (page.ids, page.total)
    };
    assert_eq!(blue(|_, _| true), (vec![11, 10], 2));
    assert_eq!(blue(|_, id| id != 11), (vec![10], 1));
    assert_eq!(blue(|_, _| false), (vec![], 0));
    assert_eq!(index.page("", Kind::Album, None, None, 10, |_, _| true).total, 0);
}

#[test]
fn a_page_continues_in_an_index_rebuilt_since() {
    let names = ["Love A", "Love B", "Love C", "Love D", "Love E"];
    let before = Index::build(names.iter().zip(1..).map(|(name, id)| doc(Kind::Track, id, name)));
    let first = before.page("love", Kind::Track, None, None, 2, |_, _| true);
    assert_eq!(first.ids, [1, 2]);
    // Since then, one before the cursor was added and one after it removed, and the documents'
    // positions all moved.
    let after = Index::build([
        doc(Kind::Track, 6, "Love AA"),
        doc(Kind::Track, 1, "Love A"),
        doc(Kind::Track, 2, "Love B"),
        doc(Kind::Track, 3, "Love C"),
        doc(Kind::Track, 5, "Love E"),
        doc(Kind::Artist, 7, "Love"),
    ]);
    let next = after.page("love", Kind::Track, None, first.next.as_ref(), 10, |_, _| true);
    assert_eq!(next.ids, [3, 5], "nothing shown twice, nothing still there passed over");
}

#[test]
fn an_empty_page_continues_after_nothing() {
    let index = library();
    let next = index.page("say it", Kind::Track, None, None, 0, |_, _| true).next;
    assert_eq!(next, None);
}

#[test]
fn matching_finds_what_a_page_finds_in_any_order() {
    let index = library();
    let keep = |_: Kind, id: i64| id != 24;
    let mut matching = index.matching("h", Kind::Track, keep);
    matching.sort_unstable();
    let mut paged = index.page("h", Kind::Track, None, None, 10, keep).ids;
    paged.sort_unstable();
    assert_eq!(matching, paged);
    assert_eq!(matching, [23, 25]);
    assert!(index.matching("", Kind::Track, keep).is_empty());
}

/// Sections as (kind, group, IDs).
fn grouped(sections: Vec<Section>) -> Vec<(Kind, Option<String>, Vec<i64>)> {
    sections.into_iter().map(|section| (section.kind, section.group, section.ids)).collect()
}

/// An index with a track, an album and a playlist called Rock, the genres Rock and Punk Rock,
/// and the mood Rock.
fn rocks() -> Index {
    Index::build([
        Document::from_track(1, "Rock".into()),
        Document::from_album(2, Some("Rock".into())).unwrap(),
        Document::from_tag(3, "genre".into(), "Rock".into(), 1),
        Document::from_tag(4, "genre".into(), "Punk Rock".into(), 1),
        Document::from_tag(5, "mood".into(), "Rock".into(), 1),
        Document::from_playlist(6, "Rock".into()),
    ])
}

#[test]
fn each_type_of_tag_is_a_section_of_its_own() {
    let index = rocks();
    let genre = || Some("genre".to_owned());
    assert_eq!(
        grouped(index.search("rock", &Kind::ALL, 10)),
        [
            // Equally exact: the more specific kind first, then the tag types by name.
            (Kind::Track, None, vec![1]),
            (Kind::Album, None, vec![2]),
            (Kind::Playlist, None, vec![6]),
            (Kind::Tag, genre(), vec![3, 4]),
            (Kind::Tag, Some("mood".to_owned()), vec![5]),
        ]
    );
    // Only Punk Rock has punk in it, so only genres are found.
    assert_eq!(grouped(index.search("punk", &Kind::ALL, 10)), [(Kind::Tag, genre(), vec![4])]);
    assert_eq!(
        grouped(index.search("rock", &[Kind::Tag], 1)),
        [(Kind::Tag, genre(), vec![3]), (Kind::Tag, Some("mood".to_owned()), vec![5]),]
    );
}

#[test]
fn a_page_reads_one_type_of_tag() {
    let index = rocks();
    let page = |group: Option<&str>| {
        let page = index.page("rock", Kind::Tag, group, None, 10, |_, _| true);
        (page.ids, page.total)
    };
    assert_eq!(page(Some("genre")), (vec![3, 4], 2));
    assert_eq!(page(Some("mood")), (vec![5], 1));
    assert_eq!(page(Some("decade")), (vec![], 0), "a type the index lacks");
    assert_eq!(page(Some("")), (vec![], 0), "the group of no group is not one");
    let mut every = index.matching("rock", Kind::Tag, |_, _| true);
    every.sort_unstable();
    assert_eq!(every, [3, 4, 5]);
}

#[test]
fn playlists_and_tags_are_searched_by_their_names() {
    assert_eq!(
        Document::from_playlist(7, "Road Trip".into()),
        Document { kind: Kind::Playlist, group: None, id: 7, name: "Road Trip".into(), weight: 0 }
    );
    assert_eq!(
        Document::from_tag(8, "mood".into(), "Calm".into(), 12),
        Document {
            kind: Kind::Tag,
            group: Some("mood".into()),
            id: 8,
            name: "Calm".into(),
            weight: 12
        }
    );
}

/// Genres with the tracks that carry them: Rock on one, and three that contain the word.
fn genres() -> Vec<Document> {
    [(1, "Rock", 1), (2, "Alt Rock", 3), (3, "Hard Rock", 12), (4, "Punk Rock", 40)]
        .map(|(id, name, tracks)| Document::from_tag(id, "genre".into(), name.into(), tracks))
        .into()
}

#[test]
fn tags_that_match_equally_well_put_the_one_with_more_music_first() {
    let index = Index::build(genres());
    // Rock is exact, so it leads however little it carries; the rest are one word more each.
    assert_eq!(index.search("rock", &[Kind::Tag], 10)[0].ids, [1, 4, 3, 2]);
    // Equal weights fall back to the name.
    let even = Index::build([
        Document::from_tag(5, "genre".into(), "Punk Rock".into(), 7),
        Document::from_tag(6, "genre".into(), "Alt Rock".into(), 7),
    ]);
    assert_eq!(even.search("rock", &[Kind::Tag], 10)[0].ids, [6, 5]);
    // Weight never reaches past the match: a name the query only begins stays behind an exact
    // one, however heavy.
    let typo = Index::build([
        Document::from_tag(7, "genre".into(), "Rockk".into(), 1_000),
        Document::from_tag(8, "genre".into(), "Rock".into(), 1),
    ]);
    assert_eq!(typo.search("rock", &[Kind::Tag], 10)[0].ids, [8, 7]);
}

#[test]
fn a_page_of_weighed_tags_continues_where_it_left_off() {
    let index = Index::build(genres());
    let first = index.page("rock", Kind::Tag, Some("genre"), None, 2, |_, _| true);
    assert_eq!(first.ids, [1, 4]);
    let rank = first.next.unwrap();
    // Since then Alt Rock outgrew Punk Rock, and Hard Rock was added to.
    let mut changed = genres();
    changed[1] = Document::from_tag(2, "genre".into(), "Alt Rock".into(), 50);
    changed[2] = Document::from_tag(3, "genre".into(), "Hard Rock".into(), 20);
    let rebuilt = Index::build(changed);
    let next = rebuilt.page("rock", Kind::Tag, Some("genre"), Some(&rank), 10, |_, _| true);
    // Alt Rock moved ahead of where the page left off, so this pass skips it, as a list does a row
    // whose sort value moved behind its cursor; nothing already shown comes again.
    assert_eq!(next.ids, [3]);
}

/// A corpus of every kind, two tag types, weights, and names that share words and tie, made the
/// same way each time.
fn corpus() -> Vec<Document> {
    let words = ["rock", "blue", "love", "the", "song", "night", "rocket", "lov"];
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as usize
    };
    (0..300)
        .map(|id| {
            let name = (0..1 + next(3)).map(|_| words[next(words.len())]).collect::<Vec<_>>();
            let name = name.join(" ");
            match id % 6 {
                0 | 1 => Document::from_track(id, name),
                2 => Document::from_album(id, Some(name)).unwrap(),
                3 => Document::from_playlist(id, name),
                4 => Document::from_tag(id, "genre".into(), name, next(4) as u64),
                _ => Document::from_tag(id, "mood".into(), name, next(4) as u64),
            }
        })
        .collect()
}

#[test]
fn indexes_searched_as_one_find_what_one_index_of_them_all_would() {
    let documents = corpus();
    let whole = Index::build(documents.clone());
    let keep = |kind: Kind, id: i64| kind != Kind::Album || id % 4 != 2;
    // Split by kind as the server splits them, and every other way.
    let splits: [fn(&Document) -> bool; 3] =
        [|doc| doc.kind == Kind::Playlist, |doc| doc.id % 2 == 0, |doc| doc.id < 10];
    for split in splits {
        let (one, other): (Vec<_>, Vec<_>) = documents.iter().cloned().partition(split);
        let (one, other) = (Index::build(one), Index::build(other));
        let union = Union(&[&one, &other]);
        for query in ["rock", "lov", "the blue", "night song", "r", "zzz"] {
            for limit in [1, 3, 50] {
                assert_eq!(
                    union.search_within(query, &Kind::ALL, limit, keep),
                    whole.search_within(query, &Kind::ALL, limit, keep),
                    "{query} {limit}"
                );
            }
            let mut matching = union.matching(query, Kind::Tag, keep);
            let mut expected = whole.matching(query, Kind::Tag, keep);
            matching.sort_unstable();
            expected.sort_unstable();
            assert_eq!(matching, expected, "{query}");
            for (kind, group) in
                [(Kind::Track, None), (Kind::Playlist, None), (Kind::Tag, Some("mood"))]
            {
                let (mut paged, mut after) = (Vec::new(), None);
                loop {
                    let page = union.page(query, kind, group, after.as_ref(), 7, keep);
                    let expected = whole.page(query, kind, group, after.as_ref(), 7, keep);
                    assert_eq!(page, expected, "{query} {kind:?}");
                    paged.extend(page.ids);
                    match page.next {
                        Some(next) => after = Some(next),
                        None => break,
                    }
                }
                let all = whole.search_within(query, &[kind], usize::MAX, keep);
                let all = all.into_iter().find(|s| s.group.as_deref() == group);
                assert_eq!(paged, all.map(|s| s.ids).unwrap_or_default(), "{query} {kind:?}");
            }
        }
    }
}

#[test]
fn a_union_of_one_is_that_index() {
    let index = Index::build(corpus());
    let union = Union(&[&index]);
    assert_eq!(
        union.search_within("rock", &Kind::ALL, 5, |_, _| true),
        index.search("rock", &Kind::ALL, 5)
    );
    assert!(Union(&[]).search_within("rock", &Kind::ALL, 5, |_, _| true).is_empty());
}

/// What a search for `query` nearly finds, of every kind, as kind and ID.
fn near(index: &Index, query: &str) -> Vec<(Kind, i64)> {
    assert!(index.search(query, &Kind::ALL, 10).is_empty(), "{query} found something");
    index.near_misses(query, &Kind::ALL, 3, |_, _| true)
}

#[test]
fn a_near_miss_may_have_a_wrong_first_letter() {
    let index = library();
    // Taylor Swift, both words matched; not Swiftly, which leaves `baylor` unmatched.
    assert_eq!(near(&index, "baylor swift"), [(Kind::Artist, 1)]);
}

#[test]
fn a_near_miss_may_have_a_typo_more() {
    let index = library();
    // Two typos in six letters: a transposed pair and a letter too many.
    assert_eq!(near(&index, "swfiit")[0], (Kind::Artist, 1));
}

#[test]
fn a_near_miss_may_leave_one_word_unmatched() {
    let index = library();
    assert_eq!(near(&index, "taylor swift live")[0], (Kind::Artist, 1));
    assert_eq!(near(&index, "the beatles live")[0], (Kind::Artist, 3));
    // A query of one word has no word to spare.
    assert_eq!(near(&index, "qqqqqq"), []);
}

#[test]
fn a_short_word_is_never_matched_more_loosely() {
    let index = library();
    // One letter, or two, would otherwise nearly find every name.
    assert_eq!(near(&index, "zz"), []);
    assert_eq!(near(&index, "x"), []);
}

#[test]
fn near_misses_of_the_same_name_are_one() {
    let index = library();
    // The track and the album called Blue fold alike, so only the track, the more specific, is
    // offered, then the album with Blue in its name.
    assert_eq!(near(&index, "bkuee"), [(Kind::Track, 21), (Kind::Album, 10)]);
}

#[test]
fn near_misses_keep_to_what_a_search_keeps_and_to_how_many() {
    let index = library();
    let keep = |kind: Kind, id: i64| !(kind == Kind::Artist && id == 1);
    // Without Taylor Swift, Swiftly is the nearest left, though it leaves a word unmatched.
    assert_eq!(index.near_misses("baylor swift", &Kind::ALL, 3, keep), [(Kind::Track, 26)]);
    assert_eq!(
        index.near_misses("baylor swift", &[Kind::Track], 3, |_, _| true),
        [(Kind::Track, 26)]
    );
    assert_eq!(index.near_misses("baylor swift", &Kind::ALL, 1, |_, _| true), [(Kind::Artist, 1)]);
    assert!(index.near_misses("", &Kind::ALL, 3, |_, _| true).is_empty());
}

#[test]
fn indexes_searched_as_one_nearly_find_what_one_index_of_them_all_would() {
    let documents = corpus();
    let whole = Index::build(documents.clone());
    let (one, other): (Vec<_>, Vec<_>) = documents.into_iter().partition(|doc| doc.id % 3 == 0);
    let (one, other) = (Index::build(one), Index::build(other));
    let union = Union(&[&one, &other]);
    for query in ["rokc lov", "nigth", "sogn blue zzz", "blux", "tje rockett"] {
        for n in [1, 3, 10] {
            assert_eq!(
                union.near_misses(query, &Kind::ALL, n, |_, _| true),
                whole.near_misses(query, &Kind::ALL, n, |_, _| true),
                "{query} {n}"
            );
        }
    }
}
