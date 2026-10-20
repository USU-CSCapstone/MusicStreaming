//! Image decodes against the real ffmpeg.

mod common;

use common::run;
use jewelcase_ffmpeg::{Config, Ffmpeg};

#[test]
fn an_animated_image_decodes_to_its_first_frame() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("animated.gif");
    // Two frames, a tenth of a second each.
    run(&["-f", "lavfi", "-i", "color=red:s=32x32:d=0.2:r=10", path.to_str().unwrap()]);
    let rgba = Ffmpeg::new(Config::default()).decode_image(&path, 8).unwrap();
    assert_eq!((rgba.width, rgba.height), (8, 8));
}
