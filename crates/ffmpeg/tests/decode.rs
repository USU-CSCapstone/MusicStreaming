//! The PCM decode against a fake `ffmpeg` that floods stderr.

use std::path::Path;

use jewelcase_ffmpeg::{Config, Error, Ffmpeg, ProbeInfo};

/// Decode `path` with `tests/fake-ffmpeg.sh` to the end, returning the samples read. A decode
/// blocked on its stderr is killed as stalled, and fails. Panics if the decode is not over after
/// 10,000 reads, far more than the fake writes.
fn decode(path: &str) -> Result<Vec<f32>, Error> {
    let ffmpeg = Ffmpeg::new(Config {
        ffmpeg: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-ffmpeg.sh"),
        ..Config::default()
    })
    .unwrap();
    let info =
        ProbeInfo { sample_rate: 44_100, channels: 2, channel_layout: None, duration_secs: None };
    let mut stream = ffmpeg.decode_pcm(Path::new(path), &info)?;
    let (mut block, mut samples) = (Vec::new(), Vec::new());
    for _ in 0..10_000 {
        if stream.read_frames(&mut block, 256)? == 0 {
            return Ok(samples);
        }
        samples.extend_from_slice(&block);
    }
    panic!("the decode of {path} did not end");
}

/// The 1000 stereo frames the fake writes: the lines `0000` to `1999` as text, read as `f32`s,
/// so no frame repeats an earlier one.
fn written() -> Vec<f32> {
    let text: String = (0..2000).map(|n| format!("{n:04}\n")).collect();
    let samples = text.as_bytes()[..8000].as_chunks::<4>().0;
    samples.iter().map(|sample| f32::from_le_bytes(*sample)).collect()
}

#[test]
fn a_decode_that_floods_stderr_finishes() {
    assert!(decode("track.flac").unwrap() == written());
}

#[test]
fn frames_split_across_reads_are_all_read() {
    assert!(decode("split.flac").unwrap() == written());
}

#[test]
fn a_partial_frame_at_the_end_is_dropped() {
    assert!(decode("trailing.flac").unwrap() == written());
}

#[test]
fn a_failed_decode_reports_the_end_of_stderr() {
    let err = decode("unsupported.flac").unwrap_err();
    assert!(err.is_unsupported_encoding(), "{err:?}");
    let Error::Failed { status, stderr } = err else { unreachable!() };
    assert_eq!(status, Some(1));
    // The last 4096 bytes, less the cut line at their start and the trailing newline.
    assert_eq!(stderr, "Header missing\n".repeat(271) + "Decoder not found");
}
