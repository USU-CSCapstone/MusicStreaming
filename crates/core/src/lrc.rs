//! LRC parsing (`requirements/tracks.md` §5).
//!
//! Turns `[mm:ss.xx]text` lines into timestamped lines, the shape the API's
//! `Lyrics.lines` carries. Lines without a timestamp are dropped from the
//! synced view; the caller keeps the full text as the plain fallback.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncedLine {
    pub start_ms: u64,
    pub text: String,
}

/// Parse LRC text. Handles several timestamps on one line, `mm:ss`,
/// `mm:ss.x`, `mm:ss.xx`, and `mm:ss.xxx`. Returns lines sorted by time.
/// Empty when nothing had a timestamp.
pub fn parse(text: &str) -> Vec<SyncedLine> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim_start();
        let mut stamps = Vec::new();
        while let Some(after) = rest.strip_prefix('[') {
            let Some(end) = after.find(']') else { break };
            match parse_stamp(&after[..end]) {
                Some(ms) => stamps.push(ms),
                None => break, // `[Chorus]` and other non-time brackets end the prefix
            }
            rest = after[end + 1..].trim_start();
        }
        if stamps.is_empty() {
            continue;
        }
        let line = rest.trim_end().to_owned();
        for ms in stamps {
            out.push(SyncedLine {
                start_ms: ms,
                text: line.clone(),
            });
        }
    }
    out.sort_by_key(|l| l.start_ms);
    out
}

fn parse_stamp(s: &str) -> Option<u64> {
    let (min, rest) = s.split_once(':')?;
    let min: u64 = min.parse().ok()?;
    let (sec, frac) = match rest.split_once('.') {
        Some((sec, frac)) => (sec, Some(frac)),
        None => (rest, None),
    };
    if sec.len() != 2 || !sec.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let sec: u64 = sec.parse().ok()?;
    if sec >= 60 {
        return None;
    }
    let frac_ms = match frac {
        None => 0,
        Some(f) if !f.is_empty() && f.len() <= 3 && f.bytes().all(|b| b.is_ascii_digit()) => {
            let n: u64 = f.parse().ok()?;
            n * 10u64.pow(3 - f.len() as u32)
        }
        Some(_) => return None,
    };
    Some(min * 60_000 + sec * 1_000 + frac_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        let lines = parse(
            "[ti:Song]\n[00:12.00]Load up on guns\n[00:15.5]Bring your friends\n[01:02]It's fun\n[Chorus]\nnot timed",
        );
        assert_eq!(
            lines,
            vec![
                SyncedLine {
                    start_ms: 12_000,
                    text: "Load up on guns".into()
                },
                SyncedLine {
                    start_ms: 15_500,
                    text: "Bring your friends".into()
                },
                SyncedLine {
                    start_ms: 62_000,
                    text: "It's fun".into()
                },
            ]
        );
    }

    #[test]
    fn repeated_stamps_and_sorting() {
        let lines = parse("[00:30.00][00:10.00]la la");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].start_ms, 10_000);
        assert_eq!(lines[1].start_ms, 30_000);
    }

    #[test]
    fn plain_text_is_empty() {
        assert!(parse("just words\nmore words").is_empty());
    }
}
