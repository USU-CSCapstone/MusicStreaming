//! Fixtures: generated audio, tags and albums, ffmpeg helpers, a library, a synchronous scan, and
//! waits on the scanner.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag, TagType};

use super::{InspectStore, InspectTrack};
use crate::library::LibraryConfig;
use crate::queue::Scanner;
use crate::scan::{Scan, ScanContext, ScanId, ScanOptions, Trigger, run_scan};
use crate::scope::Scope;
use crate::store::Store;

/// Write a stereo 16-bit WAV of a sine at `hz` and `amplitude` lasting
/// `seconds`.
pub fn write_sine_wav(path: &Path, hz: f32, amplitude: f32, seconds: f32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    fs::create_dir_all(path.parent().unwrap()).unwrap();
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

/// A tiny valid WAV: 0.2 s of quiet tone.
pub fn write_small_wav(path: &Path) {
    write_sine_wav(path, 440.0, 0.1, 0.2);
}

/// A 1x1 PNG, for sidecar art.
pub const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// Give the WAV at `path` an ID3v2 tag holding `title`, `artist` and `album`, replacing the ID3v2
/// tag it had.
pub fn tag_wav(path: &Path, title: &str, artist: &str, album: &str) {
    let mut tag = Tag::new(TagType::Id3v2);
    tag.insert_text(ItemKey::TrackTitle, title.into());
    tag.insert_text(ItemKey::TrackArtist, artist.into());
    tag.insert_text(ItemKey::AlbumTitle, album.into());
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

/// Give the WAV at `path` an ID3v2 tag holding the text `items`, replacing the ID3v2 tag it had.
pub fn tag_wav_with(path: &Path, items: &[(ItemKey, &str)]) {
    let mut tag = Tag::new(TagType::Id3v2);
    for (key, value) in items {
        tag.push(lofty::tag::TagItem::new(*key, lofty::tag::ItemValue::Text((*value).to_owned())));
    }
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

/// Write `n` small tagged WAVs to `root/artist/album`, named `01 - Track 1.wav` onwards and
/// titled `Track 1` onwards.
pub fn make_album(root: &Path, artist: &str, album: &str, n: usize) {
    for i in 1..=n {
        let p = root.join(artist).join(album).join(format!("{i:02} - Track {i}.wav"));
        write_small_wav(&p);
        tag_wav(&p, &format!("Track {i}"), artist, album);
    }
}

/// Library 1, with `root` as its one root and no excludes.
pub fn library(root: &Path) -> LibraryConfig {
    LibraryConfig { id: 1, roots: vec![root.to_path_buf()], excludes: Vec::new() }
}

/// Run one scan of `scopes` synchronously against `store`.
pub fn scan_once(
    store: &dyn Store,
    lib: &LibraryConfig,
    trigger: Trigger,
    scopes: Vec<Scope>,
) -> Scan {
    let cancel = AtomicBool::new(false);
    let options = ScanOptions::default();
    let mut scan = store.create_scan(lib.id, trigger, &scopes).unwrap();
    let ctx = ScanContext {
        store,
        library: lib,
        cancel: &cancel,
        shutdown: &AtomicBool::new(false),
        options: &options,
    };
    run_scan(&ctx, &mut scan);
    scan
}

/// Run one manual scan of every root of `lib` synchronously against `store`.
pub fn scan_library(store: &dyn Store, lib: &LibraryConfig) -> Scan {
    scan_once(store, lib, Trigger::Manual, lib.roots.iter().map(Scope::root).collect())
}

/// The tracks of `lib` not marked missing, sorted by path.
pub(super) fn live(store: &dyn InspectStore, lib: &LibraryConfig) -> Vec<InspectTrack> {
    let mut v: Vec<InspectTrack> =
        store.inspect_tracks(lib.id).into_iter().filter(|t| !t.missing).collect();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v
}

/// Makes the store a scenario runs against, holding `lib`: library 1, as [`library`] makes it,
/// with every one of its roots. It takes the whole library because a scenario may give it more
/// than one root, and a store that keeps rows for them (SQLite) creates each here, as it can
/// place a path only under a root it holds. The library's excludes reach the scan through `lib`,
/// so the store need not keep them.
pub type Factory<S> = fn(&LibraryConfig) -> S;

/// Blocks until `condition` holds for `scanner`, failing the test with `what` after 30 seconds.
pub fn wait_until(scanner: &Scanner, what: &str, condition: impl Fn(&Scanner) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition(scanner) {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Blocks until `scanner` is running the scan `id`, failing the test after 30 seconds.
pub(super) fn wait_until_running(scanner: &Scanner, id: ScanId) {
    let started = |scanner: &Scanner| scanner.running().is_some_and(|scan| scan.id == id);
    wait_until(scanner, &format!("scan {id} never started"), started);
}

/// Whether `ffmpeg` on `PATH` runs, so tests that need it can skip where it does not.
pub fn ffmpeg_available() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
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
