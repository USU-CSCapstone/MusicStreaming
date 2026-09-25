//! Shared fixtures: generated audio, tagging, and a scanner harness.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use jewelcase_scanner::scan::{ScanContext, ScanOptions, run_scan};
use jewelcase_scanner::*;

/// Write a stereo 16-bit WAV of a sine at `hz` and `amplitude` lasting
/// `seconds`.
pub fn write_sine_wav(path: &Path, hz: f32, amplitude: f32, seconds: f32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let n = (seconds * spec.sample_rate as f32) as u32;
    for i in 0..n {
        let t = i as f32 / spec.sample_rate as f32;
        let v = (amplitude * (2.0 * std::f32::consts::PI * hz * t).sin() * i16::MAX as f32) as i16;
        w.write_sample(v).unwrap();
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}

/// A tiny valid WAV: 0.2 s of quiet tone. Enough for tag tests.
pub fn write_small_wav(path: &Path) {
    write_sine_wav(path, 440.0, 0.1, 0.2);
}

pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Encode `wav` to `out` with the given ffmpeg codec arguments.
pub fn encode(wav: &Path, out: &Path, args: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(wav)
        .args(args)
        .arg(out)
        .status()
        .expect("run ffmpeg");
    assert!(status.success(), "ffmpeg failed encoding {}", out.display());
}

pub fn library(root: &Path) -> LibraryConfig {
    LibraryConfig {
        id: "lib".into(),
        roots: vec![root.to_path_buf()],
        excludes: Vec::new(),
    }
}

/// Run one scan of `scopes` synchronously against `store`.
pub fn scan_once(
    store: &MemoryStore,
    lib: &LibraryConfig,
    trigger: Trigger,
    scopes: Vec<Scope>,
) -> Scan {
    let governor = Governor::new();
    let cancel = AtomicBool::new(false);
    let options = ScanOptions::default();
    let mut scan = Scan {
        id: 0,
        library: lib.id.clone(),
        trigger,
        scopes,
        state: ScanState::Queued,
        started_at: None,
        finished_at: None,
        progress: ScanProgress::default(),
        cursor: None,
    };
    scan.id = store.create_scan(&scan);
    let ctx = ScanContext {
        store,
        library: lib,
        governor: &governor,
        cancel: &cancel,
        options: &options,
    };
    run_scan(&ctx, &mut scan);
    scan
}

pub fn scan_library(store: &MemoryStore, lib: &LibraryConfig) -> Scan {
    scan_once(
        store,
        lib,
        Trigger::Manual,
        lib.roots.iter().map(Scope::root).collect(),
    )
}

pub fn shared(store: MemoryStore) -> Arc<MemoryStore> {
    Arc::new(store)
}

pub fn paths_of(store: &MemoryStore, lib: &LibraryConfig) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = store
        .tracks(&lib.id)
        .into_iter()
        .filter(|t| !t.missing)
        .map(|t| t.record.path)
        .collect();
    v.sort();
    v
}
