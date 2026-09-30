//! Talking to LRCLIB (https://lrclib.net/docs), without any I/O: request URLs, response
//! parsing, and choosing the best match. Tested natively.

use serde::Deserialize;

pub const API: &str = "https://lrclib.net/api";

/// How far a match's length may be from the track's and still be the same recording.
pub const DURATION_TOLERANCE_S: f64 = 3.0;

pub struct Query<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: Option<&'a str>,
    pub duration_s: f64,
}

/// One LRCLIB record; the fields the plugin uses.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub instrumental: bool,
    #[serde(default)]
    pub plain_lyrics: Option<String>,
    #[serde(default)]
    pub synced_lyrics: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Found {
    Synced(String),
    Plain(String),
    Instrumental,
    Nothing,
}

/// Percent-encodes everything but RFC 3986's unreserved characters.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The exact-match lookup: track, artist, album, and duration.
pub fn get_url(q: &Query) -> String {
    let mut url = format!(
        "{API}/get?track_name={}&artist_name={}&duration={}",
        encode(q.title),
        encode(q.artist),
        q.duration_s.round() as u64
    );
    if let Some(album) = q.album {
        url.push_str(&format!("&album_name={}", encode(album)));
    }
    url
}

/// The looser lookup, when the exact one finds nothing or only plain lyrics.
pub fn search_url(q: &Query) -> String {
    format!(
        "{API}/search?track_name={}&artist_name={}",
        encode(q.title),
        encode(q.artist)
    )
}

/// `/get`: a record, or none for LRCLIB's 404.
pub fn parse_get(status: u16, body: &str) -> Result<Option<Record>, String> {
    match status {
        200 => serde_json::from_str(body)
            .map(Some)
            .map_err(|e| format!("unreadable reply: {e}")),
        404 => Ok(None),
        s => Err(format!("LRCLIB answered {s}")),
    }
}

pub fn parse_search(status: u16, body: &str) -> Result<Vec<Record>, String> {
    match status {
        200 => serde_json::from_str(body).map_err(|e| format!("unreadable reply: {e}")),
        s => Err(format!("LRCLIB answered {s}")),
    }
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Synced lyrics from the exact match or any close search result, else plain ones,
/// else whether it is an instrumental.
pub fn best(exact: Option<&Record>, search: &[Record], duration_s: f64) -> Found {
    let close: Vec<&Record> = search
        .iter()
        .filter(|r| {
            r.duration
                .is_some_and(|d| (d - duration_s).abs() <= DURATION_TOLERANCE_S)
        })
        .collect();
    let candidates = exact.into_iter().chain(close.iter().copied());
    let mut plain = None;
    let mut instrumental = false;
    for r in candidates {
        if let Some(s) = nonempty(&r.synced_lyrics) {
            return Found::Synced(s.to_owned());
        }
        if plain.is_none() {
            plain = nonempty(&r.plain_lyrics).map(str::to_owned);
        }
        instrumental |= r.instrumental;
    }
    match plain {
        Some(p) => Found::Plain(p),
        None if instrumental => Found::Instrumental,
        None => Found::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q() -> Query<'static> {
        Query {
            title: "Brandy (You're a Fine Girl)",
            artist: "Looking Glass",
            album: None,
            duration_s: 186.4,
        }
    }

    #[test]
    fn encodes_query_values() {
        assert_eq!(
            encode("Brandy (You're a Fine Girl)"),
            "Brandy%20%28You%27re%20a%20Fine%20Girl%29"
        );
        assert_eq!(encode("Devil’s Den"), "Devil%E2%80%99s%20Den");
        assert_eq!(encode("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn builds_urls() {
        assert_eq!(
            get_url(&q()),
            "https://lrclib.net/api/get?track_name=Brandy%20%28You%27re%20a%20Fine%20Girl%29&artist_name=Looking%20Glass&duration=186"
        );
        let with_album = Query {
            album: Some("Looking Glass"),
            ..q()
        };
        assert!(get_url(&with_album).ends_with("&album_name=Looking%20Glass"));
        assert!(search_url(&q()).starts_with("https://lrclib.net/api/search?track_name=Brandy"));
    }

    #[test]
    fn parses_replies() {
        let body = r#"{"id":1,"duration":193.0,"instrumental":false,"plainLyrics":"a","syncedLyrics":"[00:01.00]a"}"#;
        let r = parse_get(200, body).unwrap().unwrap();
        assert_eq!(r.synced_lyrics.as_deref(), Some("[00:01.00]a"));
        assert!(parse_get(404, r#"{"statusCode":404}"#).unwrap().is_none());
        assert!(parse_get(500, "").is_err());
        assert_eq!(parse_search(200, "[]").unwrap().len(), 0);
    }

    #[test]
    fn prefers_synced_from_a_close_search_result() {
        let exact = Record {
            plain_lyrics: Some("plain".into()),
            ..Default::default()
        };
        let search = vec![
            Record {
                duration: Some(300.0),
                synced_lyrics: Some("[00:01]far".into()),
                ..Default::default()
            },
            Record {
                duration: Some(188.0),
                synced_lyrics: Some("[00:01]near".into()),
                ..Default::default()
            },
        ];
        assert_eq!(
            best(Some(&exact), &search, 186.4),
            Found::Synced("[00:01]near".into())
        );
        assert_eq!(
            best(Some(&exact), &search[..1], 186.4),
            Found::Plain("plain".into())
        );
    }

    #[test]
    fn reports_instrumentals_and_misses() {
        let inst = Record {
            instrumental: true,
            ..Default::default()
        };
        assert_eq!(best(Some(&inst), &[], 100.0), Found::Instrumental);
        assert_eq!(best(None, &[], 100.0), Found::Nothing);
        let blank = Record {
            synced_lyrics: Some("  ".into()),
            ..Default::default()
        };
        assert_eq!(best(Some(&blank), &[], 100.0), Found::Nothing);
    }
}
