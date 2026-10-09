//! What ListenBrainz takes (https://listenbrainz.readthedocs.io/en/latest/users/api/core.html):
//! which plays count as listens, the body that submits them, and the one that says what is
//! playing now.

use serde_json::{Value, json};

pub const VALIDATE_URL: &str = "https://api.listenbrainz.org/1/validate-token";
pub const SUBMIT_URL: &str = "https://api.listenbrainz.org/1/submit-listens";

/// The most listens one submission may carry.
pub const MAX_PER_SUBMISSION: usize = 1000;

/// A play, as much of it as a listen needs.
pub struct Listen<'a> {
    pub title: &'a str,
    pub artists: &'a [String],
    pub album: Option<&'a str>,
    pub isrc: Option<&'a str>,
    pub track_number: Option<u32>,
    pub duration_ms: u64,
    /// Milliseconds since the Unix epoch.
    pub started_at: u64,
    pub listen_time_ms: u64,
}

/// Whether a play counts as a listen: ListenBrainz asks for half the track or four minutes
/// heard, whichever is less. A track of unknown length needs the four minutes.
pub fn counts(duration_ms: u64, listen_time_ms: u64) -> bool {
    let half = if duration_ms == 0 { u64::MAX } else { duration_ms / 2 };
    listen_time_ms >= half.min(4 * 60_000)
}

/// The body that submits `listens`: an import, which takes any number up to
/// [`MAX_PER_SUBMISSION`].
pub fn submission(listens: &[Listen]) -> Value {
    let payload: Vec<Value> = listens.iter().map(listen).collect();
    json!({ "listen_type": "import", "payload": payload })
}

/// The body that tells ListenBrainz `l` is playing now. ListenBrainz shows it until the track
/// would have ended, and takes it with no `listened_at`, since it is not yet a listen.
pub fn playing_now(l: &Listen) -> Value {
    json!({ "listen_type": "playing_now", "payload": [{ "track_metadata": metadata(l) }] })
}

fn listen(l: &Listen) -> Value {
    json!({ "listened_at": l.started_at / 1000, "track_metadata": metadata(l) })
}

fn metadata(l: &Listen) -> Value {
    let mut info = json!({
        "media_player": "Jewelcase",
        "submission_client": "Jewelcase ListenBrainz plugin",
        "submission_client_version": env!("CARGO_PKG_VERSION"),
        "duration_ms": l.duration_ms,
    });
    if let Some(isrc) = l.isrc {
        info["isrc"] = isrc.into();
    }
    if let Some(n) = l.track_number {
        info["tracknumber"] = n.into();
    }
    if l.artists.len() > 1 {
        info["artist_names"] = l.artists.into();
    }
    let mut metadata = json!({
        "artist_name": l.artists.join(", "),
        "track_name": l.title,
        "additional_info": info,
    });
    if let Some(album) = l.album.filter(|a| !a.is_empty()) {
        metadata["release_name"] = album.into();
    }
    metadata
}

/// Whether the answer to a token check says it is valid.
pub fn token_valid(status: u16, body: &str) -> bool {
    status == 200
        && serde_json::from_str::<Value>(body).is_ok_and(|v| v["valid"] == Value::Bool(true))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            title: "Signal",
            artists: &artists,
            album: Some("Signal"),
            isrc: Some("USRC17607839"),
            track_number: Some(3),
            duration_ms: 180_000,
            started_at: 1_790_942_400_500,
            listen_time_ms: 180_000,
        };
        let body = submission(&[listen]);
        assert_eq!(body["listen_type"], "import");
        let first = &body["payload"][0];
        assert_eq!(first["listened_at"], 1_790_942_400);
        assert_eq!(first["track_metadata"]["artist_name"], "Aurora Lane, Guest");
        assert_eq!(first["track_metadata"]["release_name"], "Signal");
        assert_eq!(first["track_metadata"]["additional_info"]["tracknumber"], 3);
        assert_eq!(first["track_metadata"]["additional_info"]["artist_names"][1], "Guest");
    }

    #[test]
    fn playing_now_names_the_track_but_not_when() {
        let artists = ["Aurora Lane".to_owned()];
        let listen = Listen {
            title: "Signal",
            artists: &artists,
            album: None,
            isrc: None,
            track_number: None,
            duration_ms: 180_000,
            started_at: 1_790_942_400_500,
            listen_time_ms: 0,
        };
        let body = playing_now(&listen);
        assert_eq!(body["listen_type"], "playing_now");
        let only = body["payload"].as_array().unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0]["track_metadata"]["track_name"], "Signal");
        assert!(only[0].get("listened_at").is_none());
    }

    #[test]
    fn a_token_is_valid_only_when_listenbrainz_says_so() {
        assert!(token_valid(200, r#"{"code":200,"valid":true,"user_name":"sam"}"#));
        assert!(!token_valid(200, r#"{"code":200,"valid":false}"#));
        assert!(!token_valid(401, r#"{"valid":true}"#));
    }
}
