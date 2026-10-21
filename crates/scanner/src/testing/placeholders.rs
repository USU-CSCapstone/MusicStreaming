//! Scenarios for image placeholders (`design/scanning.md` §12).

use std::fs;
use std::sync::Arc;

use super::{Factory, InspectStore, library, make_album, scan_library};

/// Each distinct image gets one placeholder: a few bytes of ThumbHash for one ffmpeg decodes, and
/// an empty one for one it cannot, which is not tried again.
pub fn placeholders_fill_once_per_image<S: InspectStore + 'static>(make: Factory<S>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("lib");
    // Two tracks share a red 300×200 cover; another album's cover has a valid header and
    // nothing after it, so the scanner indexes it and ffmpeg cannot decode it.
    make_album(&root, "A", "X", 2);
    make_album(&root, "B", "Y", 1);
    let cover = root.join("A/X/cover.png");
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=red:s=300x200"])
        .args(["-frames:v", "1"])
        .arg(&cover)
        .status()
        .unwrap();
    assert!(status.success());
    let png = fs::read(&cover).unwrap();
    fs::write(root.join("B/Y/cover.png"), &png[..33]).unwrap();
    let store = Arc::new(make(&root));
    let lib = library(&root);
    scan_library(&*store, &lib);

    let pending = store.next_without_placeholder(lib.id, 10).unwrap();
    assert_eq!(pending.len(), 2, "one per distinct image: {pending:?}");
    let red = pending.iter().find(|(_, file)| file.path == cover).unwrap().0.clone();
    let broken = pending.iter().find(|(hash, _)| *hash != red).unwrap().0.clone();

    let ffmpeg = jewelcase_ffmpeg::Ffmpeg::new(jewelcase_ffmpeg::Config::default());
    let placeholders = crate::placeholders::Placeholders::new(ffmpeg, store.clone());
    let report = placeholders.run_once(lib.id, 10);
    assert_eq!((report.done, report.failed), (1, 1));
    assert!(store.next_without_placeholder(lib.id, 10).unwrap().is_empty());
    assert!(placeholders.run_once(lib.id, 10).idle(), "a broken image is not retried");

    let hash = store.inspect_placeholder(lib.id, &red).unwrap();
    assert!(hash.len() < 40, "a few bytes: {}", hash.len());
    let (r, g, b, a) = thumbhash::thumb_hash_to_average_rgba(&hash).unwrap();
    assert!(r > 0.9 && g < 0.1 && b < 0.1 && a > 0.99, "{r} {g} {b} {a}");
    let ratio = thumbhash::thumb_hash_to_approximate_aspect_ratio(&hash).unwrap();
    // ThumbHash keeps the aspect ratio only coarsely.
    assert!((ratio - 1.5).abs() < 0.2, "{ratio}");
    assert_eq!(store.inspect_placeholder(lib.id, &broken), Some(Vec::new()));
}
