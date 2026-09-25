//! The one decode pass against a signal with known loudness and peak.

mod common;

use std::sync::Arc;

use common::*;
use jewelcase_ffmpeg::{Config, Ffmpeg};
use jewelcase_scanner::analysis::{ANALYZER_VERSION, Analyzer, FeatureExtractor, FeatureSink};
use jewelcase_scanner::*;

struct CountingExtractor;
struct CountingSink(u64);
impl FeatureExtractor for CountingExtractor {
    fn begin(&self, _sample_rate: u32) -> Box<dyn FeatureSink> {
        Box::new(CountingSink(0))
    }
}
impl FeatureSink for CountingSink {
    fn push_mono(&mut self, mono: &[f32]) {
        self.0 += mono.len() as u64;
    }
    fn finish(self: Box<Self>) -> Option<Vec<u8>> {
        Some(self.0.to_le_bytes().to_vec())
    }
}

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
    assert!(
        caps.missing.is_empty(),
        "missing decoders: {:?}",
        caps.missing
    );

    let store = shared(MemoryStore::new());
    let governor = Arc::new(Governor::new());
    let analyzer =
        Analyzer::new(ffmpeg, store.clone(), governor).with_features(Arc::new(CountingExtractor));

    for path in [&wav, &flac] {
        let r = analyzer.analyze_file(path).expect("analyze");
        let lufs = r.integrated_lufs.expect("loudness");
        assert!((lufs - -6.7).abs() < 1.0, "{}: {lufs} LUFS", path.display());
        let peak = r.true_peak_dbtp.expect("peak");
        assert!(
            (peak - -6.02).abs() < 0.3,
            "{}: {peak} dBTP",
            path.display()
        );
        assert_eq!(r.waveform.peaks.len(), 512);
        // Every bin sees the same amplitude.
        let mid = r.waveform.peaks[256];
        assert!((mid as i32 - 128).abs() <= 2, "peak bin {mid}");
        assert!(
            r.waveform
                .peaks
                .iter()
                .all(|p| (*p as i32 - mid as i32).abs() <= 2)
        );
        // RMS of a sine is peak / sqrt(2).
        let rms = r.waveform.rms[256] as f64 / 255.0;
        assert!((rms - 0.5 / 2f64.sqrt()).abs() < 0.02, "rms {rms}");
        // The feature sink saw every frame once.
        let frames = u64::from_le_bytes(r.features.unwrap().try_into().unwrap());
        assert!(
            (frames as i64 - 3 * 44_100).abs() < 4_096,
            "frames {frames}"
        );
        assert_eq!(r.analyzer_version, ANALYZER_VERSION);
    }
}

#[test]
fn queue_drains_and_broken_files_do_not_repeat() {
    if !ffmpeg_available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_sine_wav(&root.join("ok.wav"), 440.0, 0.2, 0.5);
    std::fs::write(
        root.join("bad.mp3"),
        b"ID3\x03\x00\x00\x00\x00\x00\x00garbage garbage garbage",
    )
    .unwrap();

    let store = shared(MemoryStore::new());
    let lib = library(root);
    scan_library(&store, &lib);
    // The garbage MP3 may or may not pass tag reading; make sure something
    // is indexed either way so analysis has work.
    let indexed = store.tracks(&lib.id).len();
    assert!(indexed >= 1);

    let ffmpeg = Ffmpeg::new(Config::default());
    let governor = Arc::new(Governor::new());
    let analyzer = Analyzer::new(ffmpeg, store.clone(), governor);
    let first = analyzer.run_once(&lib.id, 10);
    assert_eq!(first.analyzed + first.failed, indexed);
    assert!(first.analyzed >= 1);
    let second = analyzer.run_once(&lib.id, 10);
    assert_eq!(second.analyzed + second.failed, 0, "nothing left to do");
    let ok = store
        .tracks(&lib.id)
        .into_iter()
        .find(|t| t.record.path.ends_with("ok.wav"))
        .unwrap();
    assert!(ok.analysis.unwrap().integrated_lufs.is_some());
}

#[test]
fn paused_governor_holds_analysis() {
    if !ffmpeg_available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let wav = tmp.path().join("long.wav");
    write_sine_wav(&wav, 440.0, 0.2, 2.0);
    let ffmpeg = Ffmpeg::new(Config::default());
    let governor = Arc::new(Governor::new());
    let analyzer = Arc::new(Analyzer::new(
        ffmpeg,
        shared(MemoryStore::new()),
        governor.clone(),
    ));
    governor.pause();
    let a2 = analyzer.clone();
    let w = wav.clone();
    let handle = std::thread::spawn(move || a2.analyze_file(&w));
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        !handle.is_finished(),
        "analysis must not complete while paused"
    );
    governor.resume();
    let result = handle.join().unwrap().expect("analysis after resume");
    assert!(result.integrated_lufs.is_some());
}
