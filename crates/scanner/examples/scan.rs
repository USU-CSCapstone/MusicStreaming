//! Scan a folder with the in-memory store and print what was found.
//!
//!     cargo run -p jewelcase-scanner --example scan -- /path/to/music [--analyze] [--watch]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use jewelcase_ffmpeg::{Config, Ffmpeg};
use jewelcase_scanner::analysis::Analyzer;
use jewelcase_scanner::duplicates::find_duplicates;
use jewelcase_scanner::scan::ScanOptions;
use jewelcase_scanner::triggers::FsWatcher;
use jewelcase_scanner::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next().map(PathBuf::from) else {
        eprintln!("usage: scan <root> [--analyze] [--watch]");
        std::process::exit(2);
    };
    let rest: Vec<String> = args.collect();
    let analyze = rest.iter().any(|a| a == "--analyze");
    let watch = rest.iter().any(|a| a == "--watch");

    let store = Arc::new(MemoryStore::new());
    let library = LibraryConfig {
        id: "demo".into(),
        roots: vec![root.clone()],
        excludes: Vec::new(),
    };
    let governor = Arc::new(Governor::new());
    let scanner = Scanner::start(
        store.clone(),
        library.clone(),
        governor.clone(),
        ScanOptions::default(),
    );

    let started = Instant::now();
    scanner.scan_library(Trigger::Initial);
    scanner.wait_idle();
    let elapsed = started.elapsed();

    let tracks = store.tracks(&library.id);
    let scan = store.scans(&library.id).into_iter().last().unwrap();
    println!(
        "scan {:?} in {:.2?}: {:?}",
        scan.state, elapsed, scan.progress
    );
    println!("{} tracks indexed", tracks.len());

    let mut by_format = std::collections::BTreeMap::new();
    for t in &tracks {
        *by_format
            .entry(t.record.properties.format.display_name())
            .or_insert(0usize) += 1;
    }
    for (f, n) in &by_format {
        println!("  {f}: {n}");
    }
    for t in tracks.iter().take(10) {
        let r = &t.record;
        println!(
            "  {} — {} [{}] {}{}",
            r.tags.artists.join("; "),
            r.display_title(),
            r.tags.album.as_deref().unwrap_or("?"),
            match &r.artwork {
                Some(ArtworkSource::Embedded) => "art:embedded ",
                Some(ArtworkSource::Sidecar(_)) => "art:sidecar ",
                None => "",
            },
            if r.lyrics_sidecar.is_some() || r.tags.lyrics.is_some() {
                "lyrics"
            } else {
                ""
            }
        );
    }
    if tracks.len() > 10 {
        println!("  ...");
    }

    let problems = store.problems(&library.id);
    if !problems.is_empty() {
        println!("{} problems:", problems.len());
        for p in problems.iter().take(10) {
            println!("  {:?} {}: {}", p.kind, p.path.display(), p.detail);
        }
    }

    let dups = find_duplicates(store.as_ref(), &library.id);
    if !dups.is_empty() {
        println!("{} likely duplicate groups", dups.len());
    }

    if analyze {
        let ffmpeg = Ffmpeg::new(Config::default());
        match ffmpeg.verify() {
            Ok(caps) => println!(
                "ffmpeg {} ok; missing decoders: {:?}",
                caps.version, caps.missing
            ),
            Err(e) => {
                eprintln!("ffmpeg unavailable: {e}");
                std::process::exit(1);
            }
        }
        let analyzer = Analyzer::new(ffmpeg, store.clone(), governor.clone());
        let started = Instant::now();
        let report = analyzer.run_once(&library.id, usize::MAX);
        println!(
            "analysis: {} ok, {} failed in {:.2?}",
            report.analyzed,
            report.failed,
            started.elapsed()
        );
        for t in store.tracks(&library.id).iter().take(5) {
            if let Some(a) = &t.analysis {
                println!(
                    "  {}: {:.1} LUFS, {:.1} dBTP, waveform {} bins",
                    t.record.display_title(),
                    a.integrated_lufs.unwrap_or(f64::NAN),
                    a.true_peak_dbtp.unwrap_or(f64::NAN),
                    a.waveform.peaks.len()
                );
            }
        }
    }

    if watch {
        println!("watching {} (ctrl-c to stop)", root.display());
        let watcher = FsWatcher::start(scanner.clone(), &library.roots, Duration::from_secs(2))
            .expect("watch");
        let mut seen = store.scans(&library.id).len();
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let scans = store.scans(&library.id);
            for s in scans.iter().skip(seen) {
                if s.state != ScanState::Queued && s.state != ScanState::Running {
                    println!(
                        "scan {} {:?} {:?}: {:?}",
                        s.id, s.trigger, s.state, s.progress
                    );
                }
            }
            seen = scans
                .iter()
                .filter(|s| s.state != ScanState::Queued && s.state != ScanState::Running)
                .count()
                .max(seen);
            let _ = &watcher;
        }
    }
    scanner.shutdown();
}
