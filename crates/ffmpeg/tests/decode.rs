//! The PCM decode against a fake `ffmpeg` that floods stderr.

use std::path::Path;

use jewelcase_ffmpeg::{Config, Error, Ffmpeg, ProbeInfo};

/// Decode `path` with `tests/fake-ffmpeg.sh` to the end, returning the frames read. A decode
/// blocked on its stderr is killed as stalled, and fails.
fn decode(path: &str) -> Result<usize, Error> {
    let ffmpeg = Ffmpeg::new(Config {
        ffmpeg: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-ffmpeg.sh"),
        ..Config::default()
    });
    let info = ProbeInfo { sample_rate: 44_100, channels: 2, duration_secs: None };
    let mut stream = ffmpeg.decode_pcm(Path::new(path), &info)?;
    let mut block = Vec::new();
    let mut frames = 0;
    loop {
        match stream.read_frames(&mut block, 256)? {
            0 => return Ok(frames),
            n => frames += n,
        }
    }
}

#[test]
fn a_decode_that_floods_stderr_finishes() {
    assert_eq!(decode("track.flac").unwrap(), 1000);
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
