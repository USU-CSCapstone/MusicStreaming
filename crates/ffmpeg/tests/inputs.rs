//! How inputs are opened, against the real ffmpeg: names that look like options or URLs, and
//! pictures past the pixel cap.

use std::io::ErrorKind;
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::Once;

use jewelcase_ffmpeg::{Config, Error, Ffmpeg};

/// Runs the real ffmpeg with `args`, writing over its output, and asserts it succeeded.
fn run(args: &[&str]) {
    let status =
        Command::new("ffmpeg").args(["-nostdin", "-v", "error", "-y"]).args(args).status().unwrap();
    assert!(status.success(), "ffmpeg {args:?}");
}

/// Writes a one-second MP3 at `path`, named as a file whatever it looks like, with an attached
/// picture of `size` (`<width>x<height>`). The picture has one bit a pixel, so even a huge one is
/// small and quick to write.
fn track(path: &str, size: &str) {
    let picture = format!("color=black:s={size}:d=0.04");
    let (sound, picture) =
        (["-f", "lavfi", "-i", "sine=duration=1"], ["-f", "lavfi", "-i", &picture]);
    let mapping = ["-map", "0", "-map", "1", "-c:v", "png", "-pix_fmt", "monob"];
    let output = format!("file:{path}");
    let output = ["-disposition:v", "attached_pic", "-f", "mp3", &output];
    run(&[&sound[..], &picture, &mapping, &output].concat());
}

/// Probes and decodes the track at `path` to the end, returning the frames decoded.
fn decode(path: &Path) -> Result<usize, Error> {
    let ffmpeg = Ffmpeg::new(Config::default());
    let info = ffmpeg.probe(path)?;
    let mut stream = ffmpeg.decode_pcm(path, &info)?;
    let (mut block, mut frames) = (Vec::new(), 0);
    loop {
        match stream.read_frames(&mut block, 4096)? {
            0 => break,
            n => frames += n,
        }
    }
    Ok(frames)
}

/// The size of the picture at `path` once decoded to fit 8×8, or why it couldn't be.
fn picture(path: &Path) -> Result<(usize, usize), Error> {
    Ffmpeg::new(Config::default()).decode_image(path, 8).map(|rgba| (rgba.width, rgba.height))
}

/// Asserts that `result` is ffmpeg's refusal of a picture past the pixel cap.
fn assert_refused<T: std::fmt::Debug>(result: Result<T, Error>) {
    let err = result.unwrap_err();
    let Error::Failed { status: Some(_), stderr } = &err else { panic!("{err:?}") };
    assert!(stderr.contains("exceeds specified max pixel count"), "{stderr}");
}

/// Moves this test process, once, into an empty directory of its own under Cargo's scratch
/// directory. Only a relative path can be misread as an option or a URL, so these tests need one.
fn in_scratch_dir() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("ffmpeg-inputs");
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_current_dir(dir).unwrap();
    });
}

#[test]
fn a_name_starting_with_a_dash_is_a_file() {
    in_scratch_dir();
    // A bare `-` is standard input to ffmpeg, and a leading `-` is an option to ffprobe.
    track("-", "16x16");
    let frames = decode(Path::new("-")).unwrap();
    assert!(frames > 40_000, "{frames}");
    assert_eq!(picture(Path::new("-")).unwrap(), (8, 8));
}

#[test]
fn a_name_like_a_url_is_a_file() {
    in_scratch_dir();
    // A listener at the address the name points to would see any attempt to fetch it.
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    server.set_nonblocking(true).unwrap();
    let path = format!("http://{}/track.mp3", server.local_addr().unwrap());
    std::fs::create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
    track(&path, "16x16");
    let frames = decode(Path::new(&path)).unwrap();
    assert!(frames > 40_000, "{frames}");
    assert_eq!(picture(Path::new(&path)).unwrap(), (8, 8));
    let accepted = server.accept().map(|(_, peer)| peer);
    assert_eq!(accepted.unwrap_err().kind(), ErrorKind::WouldBlock, "nothing connected");
}

#[test]
fn an_image_past_the_pixel_cap_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let image = |size: &str| {
        let path = dir.path().join(format!("{size}.png"));
        let source = format!("color=black:s={size}");
        let output = path.to_str().unwrap();
        run(&["-f", "lavfi", "-i", &source, "-frames:v", "1", "-pix_fmt", "monob", output]);
        path
    };
    // 8192² is the cap itself. One row more is past it; a row, not a column, since ffmpeg
    // rounds a width up to its alignment.
    assert_eq!(picture(&image("8192x8192")).unwrap(), (8, 8));
    assert_refused(picture(&image("8192x8193")));
}

#[test]
fn a_track_whose_picture_is_past_the_pixel_cap_still_decodes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("track.mp3");
    track(path.to_str().unwrap(), "8192x8193");
    let frames = decode(&path).unwrap();
    assert!(frames > 40_000, "{frames}");
    assert_refused(picture(&path));
}
