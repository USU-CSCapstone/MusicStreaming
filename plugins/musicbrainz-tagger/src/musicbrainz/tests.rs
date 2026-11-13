use super::*;

/// A search answer shaped like MusicBrainz's, trimmed to what the plugin reads and a little
/// it ignores.
const KARMA_POLICE: &str = r#"{
  "created": "2026-10-30T12:00:00.000Z",
  "count": 2,
  "offset": 0,
  "recordings": [
    {
      "id": "8a8b4a8e-0000-0000-0000-000000000001",
      "score": 100,
      "title": "Karma Police",
      "length": 264066,
      "artist-credit": [{ "name": "Radiohead", "artist": { "id": "a74b1b7f", "name": "Radiohead" } }],
      "first-release-date": "1997-05-21",
      "releases": [
        {
          "id": "r-single",
          "title": "Karma Police",
          "status": "Official",
          "date": "1997-08-25",
          "release-group": { "primary-type": "Single" },
          "media": [{ "position": 1, "format": "CD", "track": [{ "number": "1", "title": "Karma Police" }], "track-count": 3 }]
        },
        {
          "id": "r-bootleg",
          "title": "Live at Somewhere",
          "status": "Bootleg",
          "date": "1996-01-01",
          "release-group": { "primary-type": "Album" },
          "media": [{ "position": 1, "track": [{ "number": "4" }] }]
        },
        {
          "id": "r-reissue",
          "title": "OK Computer OKNOTOK 1997 2017",
          "status": "Official",
          "date": "2017-06-23",
          "release-group": { "primary-type": "Album" },
          "media": [{ "position": 1, "track": [{ "number": "6" }] }]
        },
        {
          "id": "r-original",
          "title": "OK Computer",
          "status": "Official",
          "date": "1997-05-21",
          "release-group": { "id": "g-ok", "primary-type": "Album" },
          "media": [{ "position": 1, "track": [{ "number": "6" }] }]
        },
        {
          "id": "r-compilation",
          "title": "Now That's What I Call Music! 37",
          "status": "Official",
          "date": "1997-07-01",
          "artist-credit": [{ "name": "Various Artists" }],
          "release-group": { "primary-type": "Album", "secondary-types": ["Compilation"] },
          "media": [{ "position": 2, "track": [{ "number": "11" }] }]
        }
      ]
    },
    {
      "id": "8a8b4a8e-0000-0000-0000-000000000002",
      "score": 90,
      "title": "Karma Police (live)",
      "length": 270000,
      "artist-credit": [{ "name": "Radiohead" }],
      "releases": []
    }
  ]
}"#;

/// The two FRANCHISE recordings MusicBrainz has by Travis Scott, as it answered a search for
/// them, trimmed.
const FRANCHISE: &str = r#"{ "recordings": [
  { "title": "FRANCHISE", "length": 202794,
    "artist-credit": [{ "name": "Travis Scott", "joinphrase": " feat. " }, { "name": "Young Thug", "joinphrase": " & " }, { "name": "M.I.A." }],
    "releases": [{ "id": "single", "title": "FRANCHISE", "status": "Official", "date": "2020-09-24",
      "release-group": { "id": "g-single", "primary-type": "Single" }, "media": [{ "position": 1, "track": [{ "number": "1" }] }] }] },
  { "title": "FRANCHISE (REMIX)", "length": 207394,
    "artist-credit": [{ "name": "Travis Scott", "joinphrase": " feat. " }, { "name": "Future", "joinphrase": ", " }, { "name": "Young Thug", "joinphrase": " & " }, { "name": "M.I.A." }],
    "releases": [{ "id": "3d847ad3", "title": "FRANCHISE (REMIX)", "status": "Official", "date": "2020-10-07",
      "release-group": { "id": "g-remix", "primary-type": "Single", "secondary-types": ["Remix"] }, "media": [{ "position": 1, "track": [{ "number": "1" }] }] }] }
] }"#;

fn query(title: &str, length_ms: u64) -> Query {
    Query { title: title.into(), artist: None, length_ms, retitle: false }
}

fn tags(found: Found) -> Tags {
    let Found::Match(m) = found else { panic!("no match: {found:?}") };
    m.tags
}

#[test]
fn a_plain_title_is_searched_as_it_is() {
    assert_eq!(queries("Karma Police", 1), vec![query("Karma Police", 1)]);
    let numbered = queries("06 - Karma Police", 1);
    assert_eq!(numbered, vec![Query { retitle: true, ..query("Karma Police", 1) }]);
}

#[test]
fn a_title_naming_its_artist_is_searched_by_both_then_whole() {
    let title =
        "Travis Scott feat. Future, Young Thug & M.I.A. - FRANCHISE (REMIX - Official Audio)";
    let found = queries(title, 207_000);
    let by_artist = Query {
        title: "FRANCHISE (REMIX)".into(),
        artist: Some("Travis Scott".into()),
        length_ms: 207_000,
        retitle: true,
    };
    let whole = "Travis Scott feat. Future, Young Thug & M.I.A. - FRANCHISE (REMIX)";
    assert_eq!(found, vec![by_artist, Query { retitle: true, ..query(whole, 207_000) }]);
}

#[test]
fn encodes_query_values() {
    assert_eq!(encode("Don't Stop (Me Now)"), "Don%27t%20Stop%20%28Me%20Now%29");
    assert_eq!(encode("Sigur Rós"), "Sigur%20R%C3%B3s");
    assert_eq!(encode("a-b_c.d~e"), "a-b_c.d~e");
}

#[test]
fn builds_the_search() {
    let q = Query { artist: Some("AC/DC".into()), ..query(r#"Say "Hi" \ Bye"#, 200_000) };
    let lucene = r#"recording:"Say \"Hi\" \\ Bye" AND dur:[185000 TO 215000] AND artist:"AC/DC""#;
    let url = format!("{API}/recording?query={}&limit=50&fmt=json", encode(lucene));
    assert_eq!(search_url(&q), url);
    let by_title = search_url(&query("x", 10_000));
    assert!(by_title.contains(&encode("dur:[7000 TO 13000]")), "{by_title}");
    assert!(search_url(&query("x", 1_000)).contains(&encode("dur:[0 TO 4000]")), "no underflow");
}

#[test]
fn only_a_503_or_a_429_is_turned_away() {
    assert!(turned_away(503) && turned_away(429));
    assert!(!turned_away(200) && !turned_away(500) && !turned_away(400));
}

#[test]
fn only_a_200_is_read() {
    let why = "MusicBrainz answered 503";
    assert_eq!(parse(503, "").unwrap_err(), Failure::Unavailable(why.into()));
    assert!(matches!(parse(400, ""), Err(Failure::Failed(_))));
    assert!(matches!(parse(200, "<html>"), Err(Failure::Failed(_))));
    assert_eq!(parse(200, "{}").unwrap().len(), 0);
}

#[test]
fn chooses_the_original_official_album() {
    let recordings = parse(200, KARMA_POLICE).unwrap();
    let Found::Match(found) = choose(&query("karma police!", 263_000), &recordings) else {
        panic!("no match")
    };
    let tags = Tags {
        title: "Karma Police".into(),
        retitle: false,
        artists: vec!["Radiohead".into()],
        album_artists: vec!["Radiohead".into()],
        album: Some("OK Computer".into()),
        track: Some(6),
        disc: Some(1),
        date: Some("1997-05-21".into()),
    };
    assert_eq!(found.tags, tags);
    let covers = [
        "https://coverartarchive.org/release/r-original/front-500",
        "https://coverartarchive.org/release-group/g-ok/front-500",
    ];
    assert_eq!(found.covers, covers);
}

#[test]
fn finds_the_franchise_remix_from_a_videos_title() {
    let title =
        "Travis Scott feat. Future, Young Thug & M.I.A. - FRANCHISE (REMIX - Official Audio)";
    // Saved from a video, a few seconds longer than the release.
    let q = queries(title, 213_000).remove(0);
    let Found::Match(found) = choose(&q, &parse(200, FRANCHISE).unwrap()) else { panic!() };
    assert_eq!(found.tags.title, "FRANCHISE (REMIX)");
    assert!(found.tags.retitle);
    assert_eq!(found.tags.artists, ["Travis Scott", "Future", "Young Thug", "M.I.A."]);
    assert_eq!(found.tags.album.as_deref(), Some("FRANCHISE (REMIX)"));
    assert_eq!(found.tags.date.as_deref(), Some("2020-10-07"));
    assert_eq!(found.covers[0], "https://coverartarchive.org/release/3d847ad3/front-500");
    let by_title = query("FRANCHISE (REMIX)", 213_000);
    assert_eq!(choose(&by_title, &parse(200, FRANCHISE).unwrap()), Found::Nothing, "3 s alone");
}

#[test]
fn a_length_out_of_tolerance_or_another_title_is_nothing() {
    let recordings = parse(200, KARMA_POLICE).unwrap();
    assert_eq!(choose(&query("Karma Police", 267_100), &recordings), Found::Nothing);
    assert_eq!(choose(&query("Airbag", 264_000), &recordings), Found::Nothing);
    assert_eq!(choose(&query("Karma Police", 264_000), &[]), Found::Nothing);
}

#[test]
fn recordings_by_two_artists_are_ambiguous() {
    let body = r#"{ "recordings": [
        { "title": "Home", "length": 200000, "artist-credit": [{ "name": "Daughtry" }] },
        { "title": "Home", "length": 201000, "artist-credit": [{ "name": "daughtry" }] },
        { "title": "Home", "length": 199500, "artist-credit": [{ "name": "Michael Bublé" }] }
    ] }"#;
    let found = choose(&query("Home", 200_000), &parse(200, body).unwrap());
    assert_eq!(found, Found::Ambiguous(vec!["Daughtry".into(), "Michael Bublé".into()]));
}

#[test]
fn the_closest_in_length_gives_the_title() {
    let body = r#"{ "recordings": [
        { "title": "Song!", "length": 202000, "artist-credit": [{ "name": "A" }] },
        { "title": "song", "length": 200500, "artist-credit": [{ "name": "A" }] }
    ] }"#;
    assert_eq!(tags(choose(&query("Song", 200_000), &parse(200, body).unwrap())).title, "song");
}

#[test]
fn a_recording_on_no_release_still_names_its_artists() {
    let body = r#"{ "recordings": [{ "title": "Demo", "length": 100000,
        "artist-credit": [{ "name": "Daft Punk", "joinphrase": " & " }, { "name": "Pharrell Williams" }] }] }"#;
    let Found::Match(found) = choose(&query("Demo", 100_000), &parse(200, body).unwrap()) else {
        panic!("no match")
    };
    let tags = found.tags;
    assert_eq!(tags.artists, ["Daft Punk", "Pharrell Williams"]);
    assert_eq!(tags.album_artists, tags.artists);
    assert_eq!((tags.album, tags.track, tags.disc, tags.date), (None, None, None, None));
    assert!(found.covers.is_empty());
}
