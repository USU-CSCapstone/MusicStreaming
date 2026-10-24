use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{Context, Reason};

/// `reported` read as `T` and written back as it would be stored, or `None` if it is refused.
fn stored<T: DeserializeOwned + Serialize>(reported: Value) -> Option<Value> {
    let read = serde_json::from_value::<T>(reported).ok()?;
    Some(serde_json::to_value(read).unwrap())
}

#[test]
fn every_kind_of_context_is_kept_as_the_spec_defines_it() {
    let filter = json!({
        "genres": ["Jazz"], "genreMatch": "any", "yearFrom": 1960, "yearTo": 1969,
        "albumTypes": ["album", "ep", "single", "compilation"], "availability": "missing",
        "played": "never", "lastPlayedBefore": "2026-01-01T00:00:00.000Z",
        "lastPlayedAfter": "2025-01-01T00:00:00.000Z", "minPlayCount": 2
    });
    let sort = json!({ "field": "title", "order": "desc" });
    let kept = [
        json!({ "type": "track", "id": "1" }),
        json!({ "type": "album", "id": "101" }),
        json!({ "type": "playlist", "id": "p" }),
        json!({ "type": "tag", "id": "t" }),
        json!({ "type": "mix", "id": "m" }),
        json!({ "type": "artist", "id": "7", "scope": "topTracks" }),
        json!({ "type": "search", "query": "blue", "section": "lyrics", "within": "album:1:2",
                "filter": filter, "sort": sort }),
        json!({ "type": "search", "query": "blue", "section": "tracks" }),
        json!({ "type": "filter", "filter": filter, "sort": sort }),
        json!({ "type": "library", "sort": { "order": "asc" } }),
        json!({ "type": "library" }),
        json!({ "type": "radio", "seed": { "type": "library", "id": "1" } }),
        json!({ "type": "external", "sourceId": "s", "itemId": "i" }),
    ];
    for context in kept {
        assert_eq!(stored::<Context>(context.clone()).as_ref(), Some(&context));
    }
}

#[test]
fn a_context_keeps_nothing_the_spec_does_not_define() {
    let reported = json!({
        "type": "search", "query": "q", "section": "tracks", "extra": "x", "sort": { "by": 1 },
        "filter": { "yearFrom": 1990, "genres": null, "lastPlayedAfter": "2026-10-02T14:00:00+02:00" }
    });
    let expected = json!({
        "type": "search", "query": "q", "section": "tracks", "sort": {},
        "filter": { "yearFrom": 1990, "lastPlayedAfter": "2026-10-02T12:00:00.000Z" }
    });
    assert_eq!(stored::<Context>(reported), Some(expected));
    let artist = json!({ "type": "artist", "id": "7" });
    assert_eq!(
        stored::<Context>(artist),
        Some(json!({ "type": "artist", "id": "7", "scope": "all" })),
        "the spec's default scope"
    );
}

#[test]
fn a_context_the_spec_does_not_allow_is_refused() {
    let refused = [
        json!({}),
        json!("album"),
        json!({ "type": "podcast", "id": "1" }),
        json!({ "type": "album" }),
        json!({ "type": "track" }),
        json!({ "type": "album", "id": 101 }),
        json!({ "type": "artist", "id": "7", "scope": "everything" }),
        json!({ "type": "search", "query": "q" }),
        json!({ "type": "search", "query": "q", "section": "albums" }),
        json!({ "type": "filter", "filter": { "albumTypes": ["mixtape"] } }),
        json!({ "type": "filter", "filter": { "yearFrom": 1990.5 } }),
        json!({ "type": "filter", "filter": { "genres": [1] } }),
        json!({ "type": "filter", "filter": { "lastPlayedBefore": "yesterday" } }),
        json!({ "type": "library", "sort": { "order": "up" } }),
        json!({ "type": "radio", "seed": { "type": "genre", "id": "1" } }),
        json!({ "type": "radio" }),
        json!({ "type": "external", "sourceId": "s" }),
    ];
    for context in refused {
        assert_eq!(stored::<Context>(context.clone()), None, "{context}");
    }
}

#[test]
fn a_reason_keeps_only_what_the_spec_defines() {
    let reported = json!({
        "kind": "radioSeed", "text": "Radio from Björk", "extra": 1,
        "refs": [{ "type": "artist", "id": "7", "name": "Björk" }],
        "since": "2026-10-01T00:00:00-02:00"
    });
    let expected = json!({
        "kind": "radioSeed", "text": "Radio from Björk",
        "refs": [{ "type": "artist", "id": "7" }], "since": "2026-10-01T02:00:00.000Z"
    });
    assert_eq!(stored::<Reason>(reported), Some(expected));
    let plain = json!({ "kind": "becauseYouListened", "text": "Because you listened" });
    assert_eq!(stored::<Reason>(plain.clone()), Some(plain));
    for refused in [
        json!({ "kind": "radioSeed" }),
        json!({ "kind": "whim", "text": "t" }),
        json!({ "kind": "decade", "text": "t", "since": "1990s" }),
        json!({ "type": "radio", "seed": { "trackId": "1" } }),
    ] {
        assert_eq!(stored::<Reason>(refused.clone()), None, "{refused}");
    }
}

/// The spec's reason kinds, entity types and artist scopes, and the filter names that
/// `every_kind_of_context_is_kept_as_the_spec_defines_it` leaves out, each kept as reported.
#[test]
fn every_name_the_spec_allows_is_kept() {
    let kinds = [
        "becauseYouListened",
        "similarTo",
        "notHeardSince",
        "neverPlayed",
        "anniversary",
        "collaborator",
        "radioSeed",
        "endlessPlay",
        "timeOfDay",
        "decade",
        "mood",
    ];
    for kind in kinds {
        let reason = json!({ "kind": kind, "text": "t" });
        assert_eq!(stored::<Reason>(reason.clone()), Some(reason), "{kind}");
    }
    for kind in ["track", "album", "artist", "playlist", "tag", "mix", "library"] {
        let radio = json!({ "type": "radio", "seed": { "type": kind, "id": "1" } });
        assert_eq!(stored::<Context>(radio.clone()), Some(radio), "{kind}");
    }
    for scope in ["all", "discography", "appearances", "topTracks"] {
        let artist = json!({ "type": "artist", "id": "7", "scope": scope });
        assert_eq!(stored::<Context>(artist.clone()), Some(artist), "{scope}");
    }
    for (field, value) in [("genreMatch", "all"), ("availability", "available"), ("played", "ever")]
    {
        let context = json!({ "type": "filter", "filter": { field: value } });
        assert_eq!(stored::<Context>(context.clone()), Some(context), "{field}");
    }
}

/// `context` with the string or list at `pointer` replaced by `value`, then whether it is kept.
fn kept_with(mut context: Value, pointer: &str, value: Value) -> bool {
    *context.pointer_mut(pointer).unwrap() = value;
    stored::<Context>(context).is_some()
}

/// IDs, names and other short strings are at most 100 characters, a query 500, and a list at
/// most 100 items, so no report stores more than that.
#[test]
fn a_context_is_kept_up_to_its_limits_and_refused_past_them() {
    let short = |n| json!("é".repeat(n));
    let list = |n| json!(vec!["g"; n]);
    let search = json!({ "type": "search", "query": "q", "section": "tracks", "within": "w",
                         "filter": { "genres": [], "albumTypes": [] }, "sort": { "field": "f" } });
    let cases = [
        (json!({ "type": "album", "id": "1" }), "/id", 100, short as fn(usize) -> Value),
        (json!({ "type": "artist", "id": "7" }), "/id", 100, short),
        (search.clone(), "/query", 500, short),
        (search.clone(), "/within", 100, short),
        (search.clone(), "/sort/field", 100, short),
        (search.clone(), "/filter/genres", 100, list),
        (search.clone(), "/filter/genres", 100, |n| json!(["é".repeat(n)])),
        (search, "/filter/albumTypes", 100, |n| json!(vec!["ep"; n])),
        (
            json!({ "type": "radio", "seed": { "type": "track", "id": "1" } }),
            "/seed/id",
            100,
            short,
        ),
        (json!({ "type": "external", "sourceId": "s", "itemId": "i" }), "/sourceId", 100, short),
        (json!({ "type": "external", "sourceId": "s", "itemId": "i" }), "/itemId", 100, short),
    ];
    for (context, pointer, limit, value) in cases {
        assert!(kept_with(context.clone(), pointer, value(limit)), "{context} {pointer} at limit");
        assert!(!kept_with(context.clone(), pointer, value(limit + 1)), "{context} {pointer}");
    }
}

/// A reason's text is at most 500 characters, and it names at most 100 entities, each by an ID
/// of at most 100.
#[test]
fn a_reason_is_kept_up_to_its_limits_and_refused_past_them() {
    let entity = |id: String| json!({ "type": "track", "id": id });
    let reason =
        |text: String, refs: Vec<Value>| json!({ "kind": "mood", "text": text, "refs": refs });
    let kept = |reason: Value| stored::<Reason>(reason).is_some();
    assert!(kept(reason("é".repeat(500), vec![entity("é".repeat(100)); 100])));
    assert!(!kept(reason("é".repeat(501), vec![])), "text");
    assert!(!kept(reason("t".into(), vec![entity("1".into()); 101])), "refs");
    assert!(!kept(reason("t".into(), vec![entity("é".repeat(101))])), "a ref's ID");
}
