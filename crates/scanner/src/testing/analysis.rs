//! Scenarios for the analysis queue as a whole (`design/scanning.md` §12).

use std::fs;
use std::sync::Arc;

use jewelcase_ffmpeg::{Config, Ffmpeg};

use super::{Factory, InspectStore, library, scan_library, write_sine_wav};
use crate::analysis::Analyzer;
use crate::worker::Stop;

/// One pass analyzes every indexed track, judging a broken one, and the next finds nothing left:
/// a broken file is not decoded again. The good file's loudness is measured.
pub fn queue_drains_and_broken_files_do_not_repeat<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    write_sine_wav(&root.join("ok.wav"), 440.0, 0.2, 0.5);
    fs::write(root.join("bad.mp3"), b"ID3\x03\x00\x00\x00\x00\x00\x00garbage garbage garbage")
        .unwrap();

    let lib = library(&root);
    let store = Arc::new(make(&lib));
    scan_library(store.as_ref(), &lib);
    // The garbage MP3 may or may not pass tag reading; make sure something
    // is indexed either way so analysis has work.
    let indexed = store.inspect_tracks(lib.id).len();
    assert!(indexed >= 1);

    let analyzer = Analyzer::new(Ffmpeg::new(Config::default()).unwrap(), store.clone());
    let first = analyzer.run_once(lib.id, 10, &Stop::never());
    assert_eq!(first.done + first.failed, indexed);
    assert!(first.done >= 1);
    let second = analyzer.run_once(lib.id, 10, &Stop::never());
    assert_eq!(second.done + second.failed, 0, "nothing left to do");
    let tracks = store.inspect_tracks(lib.id);
    assert!(tracks.into_iter().find(|t| t.path.ends_with("ok.wav")).unwrap().measured);
}
