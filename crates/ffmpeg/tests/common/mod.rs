//! Helpers shared by the tests that run the real ffmpeg.

use std::process::Command;

/// Runs the real ffmpeg with `args`, writing over its output, and asserts it succeeded.
pub fn run(args: &[&str]) {
    let status =
        Command::new("ffmpeg").args(["-nostdin", "-v", "error", "-y"]).args(args).status().unwrap();
    assert!(status.success(), "ffmpeg {args:?}");
}
