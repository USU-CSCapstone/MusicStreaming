//! LRC parsing (`requirements/tracks.md` §5).
//!
//! Turns `[mm:ss.xx]text` lines into timestamped lines, the shape the API's
//! `Lyrics.lines` carries. Lines without a timestamp are dropped; text with
//! none at all is plain lyrics (`Lyrics::parse`).

use serde::{Deserialize, Serialize};

/// The most a parse returns, counting each line and its text. Every stamp copies its line's
/// text, so without a bound thousands of stamps before a long text would cost gigabytes; this is
/// far more than any song's lyrics need.
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncedLine {
    pub start_ms: u64,
    pub text: String,
}

/// Parse LRC text that has no byte order mark ([`crate::Lyrics::parse`] strips it). Handles
/// several timestamps on one line, `mm:ss`, and fractions of one to three digits after a `.` or
/// a `:`. An `[offset:ms]` tag, wherever it is, shifts every line: a positive offset shows them
/// sooner, never before the start; when there are several, the last one counts. Lines that would
/// take the output past [`MAX_OUTPUT_BYTES`] are left out. Returns lines sorted by time. Empty
/// when nothing had a timestamp.
pub fn parse(text: &str) -> Vec<SyncedLine> {
    let mut out = Vec::new();
    let mut offset = 0;
    let mut size = 0;
    for raw in text.lines() {
        let mut rest = raw.trim();
        if let Some(ms) = parse_offset(rest) {
            offset = ms;
            continue;
        }
        let mut stamps = Vec::new();
        while let Some(after) = rest.strip_prefix('[') {
            let Some(end) = after.find(']') else { break };
            match parse_stamp(&after[..end]) {
                Some(ms) => stamps.push(ms),
                None => break, // `[Chorus]` and other non-time brackets end the prefix
            }
            rest = after[end + 1..].trim_start();
        }
        for ms in stamps {
            size += size_of::<SyncedLine>() + rest.len();
            if size <= MAX_OUTPUT_BYTES {
                out.push(SyncedLine { start_ms: ms, text: rest.to_owned() });
            }
        }
    }
    for line in &mut out {
        line.start_ms = line.start_ms.saturating_sub_signed(offset);
    }
    out.sort_by_key(|l| l.start_ms);
    out
}

/// The milliseconds of an `[offset:+500]` line, if it is one.
fn parse_offset(line: &str) -> Option<i64> {
    line.strip_prefix("[offset:")?.strip_suffix(']')?.trim().parse().ok()
}

/// The milliseconds of an `mm:ss`, `mm:ss.xxx` or `mm:ss:xxx` stamp. `None` when it is not one,
/// including a stamp too large to count in milliseconds.
fn parse_stamp(s: &str) -> Option<u64> {
    let (min, rest) = s.split_once(':')?;
    let (sec, frac) = match rest.split_once(['.', ':']) {
        Some((sec, frac)) => (sec, Some(frac)),
        None => (rest, None),
    };
    if sec.len() != 2 {
        return None;
    }
    let sec = digits(sec).filter(|sec| *sec < 60)?;
    let frac_ms = match frac {
        None => 0,
        Some(f) if f.len() <= 3 => digits(f)? * 10u64.pow(3 - f.len() as u32),
        Some(_) => return None,
    };
    digits(min)?.checked_mul(60_000)?.checked_add(sec * 1_000 + frac_ms)
}

/// `s` as a number, if it is one or more ASCII digits and nothing else (no sign, no spaces) and
/// fits in a `u64`.
fn digits(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
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
                SyncedLine { start_ms: 12_000, text: "Load up on guns".into() },
                SyncedLine { start_ms: 15_500, text: "Bring your friends".into() },
                SyncedLine { start_ms: 62_000, text: "It's fun".into() },
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
    fn colon_fractions() {
        let lines = parse("[00:12:50]Hundredths\n[01:02:5]Tenths\n[00:03:123]Thousandths");
        let starts: Vec<u64> = lines.iter().map(|l| l.start_ms).collect();
        assert_eq!(starts, [3_123, 12_500, 62_500]);
    }

    #[test]
    fn a_positive_offset_shows_lines_sooner() {
        let lines = parse("[ar:Band]\n[offset:+500]\n[00:10.00]Ten\n[00:00.20]Start");
        let starts: Vec<u64> = lines.iter().map(|l| l.start_ms).collect();
        // Never before the start of the track.
        assert_eq!(starts, [0, 9_500]);
    }

    #[test]
    fn a_negative_offset_shows_lines_later() {
        let lines = parse("[00:10.00]Ten\n[offset: -250]");
        assert_eq!(lines, [SyncedLine { start_ms: 10_250, text: "Ten".into() }]);
    }

    #[test]
    fn a_malformed_offset_is_ignored() {
        assert_eq!(parse("[offset:soon]\n[00:10.00]Ten")[0].start_ms, 10_000);
    }

    #[test]
    fn extreme_offsets_saturate() {
        assert_eq!(parse("[offset:9223372036854775807]\n[00:10.00]Ten")[0].start_ms, 0);
        assert_eq!(
            parse("[offset:-9223372036854775808]\n[00:10.00]Ten")[0].start_ms,
            10_000 + i64::MIN.unsigned_abs()
        );
    }

    #[test]
    fn an_out_of_range_stamp_is_not_a_stamp() {
        let one = [SyncedLine { start_ms: 1_000, text: "One".into() }];
        // The first stamp's minutes overflow alone; the second's only once its seconds are added.
        assert_eq!(parse("[99999999999999999:00]Never\n[00:01.00]One"), one);
        assert_eq!(parse("[307445734561825:59.999]Never\n[00:01.00]One"), one);
    }

    #[test]
    fn signed_minutes_and_one_digit_seconds_are_not_stamps() {
        assert!(parse("[+1:00.00]Signed").is_empty());
        assert!(parse("[00:5.00]x").is_empty());
    }

    #[test]
    fn trailing_whitespace_is_trimmed() {
        assert_eq!(
            parse("[00:01.00]Hi \t\n[offset:500] "),
            [SyncedLine { start_ms: 500, text: "Hi".into() }]
        );
    }

    #[test]
    fn the_last_offset_wins() {
        assert_eq!(parse("[offset:+500]\n[offset:-500]\n[00:10.00]Ten")[0].start_ms, 10_500);
    }

    #[test]
    fn many_stamps_on_a_long_line_stay_within_the_budget() {
        let line = "[00:01.00]".repeat(2_000) + &"x".repeat(20_000);
        let lines = parse(&line);
        assert!(!lines.is_empty());
        let size: usize = lines.iter().map(|l| l.text.len() + size_of::<SyncedLine>()).sum();
        assert!(size <= MAX_OUTPUT_BYTES, "{size} bytes");
        // Each line counts, even with no text.
        let empty = parse(&"[0:00]".repeat(1_000_000));
        assert_eq!(empty.len(), MAX_OUTPUT_BYTES / size_of::<SyncedLine>());
        // An offset after the budget still applies.
        let shifted = parse(&("[00:05]".repeat(2_000) + &"x".repeat(20_000) + "\n[offset:1000]"));
        assert_eq!(shifted[0].start_ms, 4_000);
    }

    #[test]
    fn plain_text_is_empty() {
        assert!(parse("just words\nmore words").is_empty());
    }
}
