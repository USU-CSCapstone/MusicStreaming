use super::*;

pub(super) fn doc(kind: Kind, id: i64, name: &str) -> Document {
    Document { kind, id, name: name.to_owned() }
}

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
    assert_eq!(
        index.search("blue", &[Kind::Album], 10),
        [Section { kind: Kind::Album, ids: vec![11, 10], total: 2 }]
    );
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
