//! The shape of a PCM decode, against the real ffmpeg: always the channel layout and sample rate
//! the probe reported, whatever the stream does along the way.

mod common;

use std::path::Path;

use common::run;
use jewelcase_ffmpeg::{Config, Ffmpeg, ProbeInfo};

/// Probes and decodes `path` to the end, returning the probe and every sample, interleaved.
/// Panics if the decode is not over after 10,000 reads, far more than any test file here needs.
fn decode(path: &Path) -> (ProbeInfo, Vec<f32>) {
    let ffmpeg = Ffmpeg::new(Config::default()).unwrap();
    let info = ffmpeg.probe(path).unwrap();
    let mut stream = ffmpeg.decode_pcm(path, &info).unwrap();
    let (mut block, mut samples) = (Vec::new(), Vec::new());
    for _ in 0..10_000 {
        if stream.read_frames(&mut block, 1000).unwrap() == 0 {
            return (info, samples);
        }
        samples.extend_from_slice(&block);
    }
    panic!("the decode of {path:?} did not end");
}

/// Writes two seconds of a sine as a WAV file at `path`, in `layout`, which has `channels`
/// channels; channel `n` carries the sine at `n + 1` tenths of its level.
fn sine(path: &Path, layout: &str, channels: usize) {
    let gains: String = (0..channels).map(|n| format!("|c{n}=0.{}*c0", n + 1)).collect();
    let pan = format!("pan={layout}{gains}");
    let path = path.to_str().unwrap();
    run(&["-f", "lavfi", "-i", "sine=duration=2", "-af", &pan, "-c:a", "pcm_f32le", path]);
}

/// Asserts that channel `n` of `samples` peaks at `n + 1` times channel 0's peak, as [`sine`]
/// wrote them, so no channel was mixed into another or shifted along.
fn assert_channels_kept(samples: &[f32], channels: usize) {
    let mut peaks = vec![0.0f32; channels];
    for frame in samples.chunks_exact(channels) {
        for (peak, sample) in peaks.iter_mut().zip(frame) {
            *peak = peak.max(sample.abs());
        }
    }
    for (n, peak) in peaks.iter().enumerate() {
        let ratio = peak / peaks[0];
        assert!((ratio - (n + 1) as f32).abs() < 0.02, "channel peaks {peaks:?}");
    }
}

#[test]
fn a_layout_other_than_the_default_for_its_channel_count_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("track.wav");
    // ffmpeg's default six-channel layout is 5.1, so a pin to six channels alone would remix
    // this one. Six channels make 24-byte frames, which pipe reads end part way through.
    sine(&path, "6.0", 6);
    let (info, samples) = decode(&path);
    assert_eq!((info.channels, info.channel_layout.as_deref()), (6, Some("6.0")));
    assert_eq!(samples.len(), 2 * 44_100 * 6);
    assert_channels_kept(&samples, 6);
}

#[test]
fn a_stream_with_no_layout_keeps_its_channels() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("track.wav");
    // A WAV file of one or two channels names no layout.
    sine(&path, "stereo", 2);
    let (info, samples) = decode(&path);
    assert_eq!((info.channels, info.channel_layout), (2, None));
    assert_eq!(samples.len(), 2 * 44_100 * 2);
    assert_channels_kept(&samples, 2);
}

#[test]
fn a_stream_that_changes_shape_decodes_in_the_probed_shape() {
    let dir = tempfile::tempdir().unwrap();
    // A second of mono at 22.05 kHz, then a second of stereo at 44.1 kHz, end to end in one MP3.
    let parts = [("22050", "1"), ("44100", "2")].map(|(rate, channels)| {
        let part = dir.path().join(format!("{rate}.mp3"));
        let source = format!("sine=duration=1:sample_rate={rate}");
        run(&["-f", "lavfi", "-i", &source, "-ac", channels, part.to_str().unwrap()]);
        std::fs::read(part).unwrap()
    });
    let path = dir.path().join("track.mp3");
    std::fs::write(&path, parts.concat()).unwrap();
    let (info, samples) = decode(&path);
    // ffprobe reports the second part's shape, while ffmpeg starts decoding in the first's.
    assert_eq!((info.sample_rate, info.channels), (44_100, 2));
    let seconds = samples.len() as f64 / 2.0 / 44_100.0;
    assert!((1.9..2.3).contains(&seconds), "{seconds} seconds");
}
