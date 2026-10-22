//! The one decode pass against a signal with known loudness and peak.

use std::sync::Arc;

use jewelcase_core::waveform_level;
use jewelcase_ffmpeg::{Config, Ffmpeg};
use jewelcase_scanner::testing::{BareStore, encode, ffmpeg_available, write_sine_wav};
use jewelcase_scanner::{ANALYZER_VERSION, Analyzer};

#[test]
fn sine_measures_as_expected() {
    if !ffmpeg_available() {
        eprintln!("skipping analysis test: ffmpeg not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("sine.wav");
    // 1 kHz at half scale: -6.02 dBFS peak per channel. EBU R128 sums the
    // channels' mean-square power (-6.02 dB) and subtracts 0.691, so a
    // stereo half-scale sine lands near -6.7 LUFS before K-weighting's
    // small gain at 1 kHz.
    write_sine_wav(&wav, 1000.0, 0.5, 3.0);
    let flac = tmp.path().join("sine.flac");
    encode(&wav, &flac, &["-c:a", "flac"]);

    let ffmpeg = Ffmpeg::new(Config::default());
    let caps = ffmpeg.verify().expect("ffmpeg verify");
    assert!(caps.missing.is_empty(), "missing decoders: {:?}", caps.missing);

    let analyzer = Analyzer::new(ffmpeg, Arc::new(BareStore::default()));

    for path in [&wav, &flac] {
        let r = analyzer.analyze_file(path).expect("analyze");
        let lufs = r.integrated_lufs.expect("loudness");
        assert!((lufs - -6.7).abs() < 1.0, "{}: {lufs} LUFS", path.display());
        let peak = r.true_peak_dbtp.expect("peak");
        assert!((peak - -6.02).abs() < 0.3, "{}: {peak} dBTP", path.display());
        assert_eq!(r.waveform.peaks.len(), 512);
        // Every bin sees the same amplitude, stored as its level in half decibels.
        let mid = r.waveform.peaks[256];
        assert!(mid.abs_diff(waveform_level(0.5)) <= 1, "peak bin {mid}");
        assert!(r.waveform.peaks.iter().all(|p| p.abs_diff(mid) <= 1));
        // RMS of a sine is peak / sqrt(2), 3 dB below it.
        let rms = r.waveform.rms[256];
        assert!(rms.abs_diff(waveform_level(0.5 / 2f64.sqrt())) <= 1, "rms bin {rms}");
        assert_eq!(r.analyzer_version, ANALYZER_VERSION);
    }
}
