use super::*;

/// A listen of `title` by `artists`, three minutes long and heard to the end.
fn heard<'a>(title: &'a str, artists: &'a [String]) -> Listen<'a> {
    Listen {
        title,
        artists,
        album: None,
        isrc: None,
        track_number: None,
        duration_ms: 180_000,
        started_at: 1_790_942_400_500,
        listen_time_ms: 180_000,
    }
}

#[test]
fn half_the_track_or_four_minutes_counts() {
    assert!(counts(180_000, 90_000));
    assert!(!counts(180_000, 89_999));
    assert!(counts(600_000, 240_000), "four minutes of a long track");
    assert!(!counts(0, 239_999), "unknown length needs four minutes");
}

#[test]
fn a_submission_names_the_track_and_when_it_started() {
    let artists = ["Aurora Lane".to_owned(), "Guest".to_owned()];
    let listen = Listen {
        album: Some("Signal"),
        isrc: Some("USRC17607839"),
        track_number: Some(3),
        ..heard("Signal", &artists)
    };
    let body = submission(&[listen]);
    assert_eq!(body["listen_type"], "import");
    let first = &body["payload"][0];
    assert_eq!(first["listened_at"], 1_790_942_400);
    assert_eq!(first["track_metadata"]["artist_name"], "Aurora Lane, Guest");
    assert_eq!(first["track_metadata"]["release_name"], "Signal");
    let info = &first["track_metadata"]["additional_info"];
    assert_eq!(info["tracknumber"], 3);
    assert_eq!(info["artist_names"][1], "Guest");
    assert_eq!(info["duration_ms"], 180_000);
}

#[test]
fn playing_now_names_the_track_but_not_when() {
    let artists = ["Aurora Lane".to_owned()];
    let body = playing_now(&Listen { listen_time_ms: 0, ..heard("Signal", &artists) });
    assert_eq!(body["listen_type"], "playing_now");
    let only = body["payload"].as_array().unwrap();
    assert_eq!(only.len(), 1);
    assert_eq!(only[0]["track_metadata"]["track_name"], "Signal");
    assert!(only[0].get("listened_at").is_none());
}

#[test]
fn an_unknown_length_is_left_out() {
    let artists = ["Aurora Lane".to_owned()];
    let unknown = Listen { duration_ms: 0, ..heard("Signal", &artists) };
    let submitted = &submission(std::slice::from_ref(&unknown))["payload"][0];
    let now = &playing_now(&unknown)["payload"][0];
    for body in [submitted, now] {
        let info = body["track_metadata"]["additional_info"].as_object().unwrap();
        assert!(!info.contains_key("duration_ms"), "{info:?}");
    }
}

#[test]
fn a_blank_title_or_artist_or_a_nul_is_not_sendable() {
    let artist = ["Aurora Lane".to_owned()];
    assert!(sendable(&heard("Signal", &artist)));
    assert!(!sendable(&heard("Signal", &[])), "no artists");
    assert!(!sendable(&heard("Signal", &["  ".to_owned()])), "a blank artist");
    assert!(!sendable(&heard(" \t", &artist)), "a blank title");
    assert!(!sendable(&heard("Sig\0nal", &artist)), "a NUL in the title");
    assert!(!sendable(&heard("Signal", &["Aurora\0".to_owned()])), "a NUL in an artist");
}

#[test]
fn an_album_or_isrc_holding_a_nul_is_left_out() {
    let artists = ["Aurora Lane".to_owned()];
    let listen =
        Listen { album: Some("Sig\0nal"), isrc: Some("US\0"), ..heard("Signal", &artists) };
    assert!(sendable(&listen));
    let metadata = &submission(&[listen])["payload"][0]["track_metadata"];
    assert!(metadata.get("release_name").is_none(), "{metadata}");
    assert!(metadata["additional_info"].get("isrc").is_none(), "{metadata}");
}

/// What `submit` came to with a ListenBrainz that refuses any batch holding a listen titled
/// "Bad" with `refusal`, and, given `fail_at` as `(n, status)`, answers every request from the
/// nth on with `status`: the result, and how many listens each request carried.
fn submit_to(
    titles: &[&str],
    refusal: u16,
    fail_at: Option<(usize, u16)>,
) -> (Result<usize, String>, Vec<usize>) {
    let artists = ["Aurora Lane".to_owned()];
    let listens: Vec<Listen> = titles.iter().map(|t| heard(t, &artists)).collect();
    let mut sizes = Vec::new();
    let result = submit(&listens, |batch| {
        sizes.push(batch.len());
        match fail_at {
            Some((at, status)) if sizes.len() >= at => Ok((status, "try later".into())),
            _ if batch.iter().any(|l| l.title == "Bad") => Ok((refusal, " invalid \n".into())),
            _ => Ok((200, r#"{"status":"ok"}"#.into())),
        }
    });
    (result, sizes)
}

#[test]
fn listens_go_a_thousand_at_a_time() {
    let (result, sizes) = submit_to(&["Good"; 1_001], 400, None);
    assert_eq!(result.unwrap(), 0);
    assert_eq!(sizes, [1_000, 1]);
}

#[test]
fn a_refused_batch_is_halved_until_the_bad_listen_is_found() {
    let mut titles = vec!["Good"; 100];
    titles[0] = "Bad";
    let (result, sizes) = submit_to(&titles, 400, None);
    assert_eq!(result.unwrap(), 1);
    assert_eq!(sizes, [100, 50, 25, 12, 6, 3, 1, 2, 3, 6, 13, 25, 50]);
}

#[test]
fn every_listen_refused_costs_at_most_twice_as_many_requests() {
    let (result, sizes) = submit_to(&["Bad"; 4], 400, None);
    assert_eq!(result.unwrap(), 4);
    assert_eq!(sizes.len(), 2 * 4 - 1, "{sizes:?}");
}

#[test]
fn a_refused_single_listen_is_left_out_rather_than_kept() {
    let (result, sizes) = submit_to(&["Bad"], 400, None);
    assert_eq!(result.unwrap(), 1);
    assert_eq!(sizes, [1], "not sent twice");
}

#[test]
fn other_refusals_and_failures_keep_the_plays() {
    for status in [401, 429, 500, 503] {
        let (result, sizes) = submit_to(&["One", "Bad"], status, None);
        assert_eq!(result.unwrap_err(), format!("ListenBrainz answered {status}: invalid"));
        assert_eq!(sizes.len(), 1, "{status}: not split");
    }
    // A failure while sending the halves keeps them all, too.
    let (result, sizes) = submit_to(&["Bad", "One"], 400, Some((3, 503)));
    assert_eq!(result.unwrap_err(), "ListenBrainz answered 503: try later");
    assert_eq!(sizes, [2, 1, 1]);
    // And so does a request that never got an answer.
    let artists = ["Aurora Lane".to_owned()];
    let failed = submit(&[heard("One", &artists)], |_| Err("timed out".to_owned()));
    assert_eq!(failed.unwrap_err(), "timed out");
}

#[test]
fn the_summary_counts_what_became_of_each_play() {
    let all = Delivery { plays: 10, short: 2, unnamed: 1, rejected: 3 };
    assert_eq!(
        all.summary(),
        "Sent 4 listens; 2 plays were too short to count, 1 had no usable title or artist, \
         ListenBrainz turned away 3."
    );
    let sent = Delivery { plays: 3, short: 0, unnamed: 0, rejected: 0 };
    assert_eq!(sent.summary(), "Sent 3 listens; 0 plays were too short to count.");
}

#[test]
fn a_token_is_valid_only_when_listenbrainz_says_so() {
    assert!(token_valid(200, r#"{"code":200,"valid":true,"user_name":"sam"}"#));
    assert!(!token_valid(200, r#"{"code":200,"valid":false}"#));
    assert!(!token_valid(401, r#"{"valid":true}"#));
}
