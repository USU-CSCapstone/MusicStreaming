//! The host's rules, kept apart from Wasmtime so they are testable: which destinations a
//! plugin may reach, where lyrics go, and that nothing existing is ever overwritten.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The scanner's lyrics extensions, synced first (`crates/scanner/src/sidecar.rs`).
const LYRICS_EXTENSIONS: [&str; 2] = ["lrc", "txt"];

/// Whether `url` may be fetched, given the destinations the manifest declared. Only
/// http(s), and only to a listed host (or any, for `*`); subdomains must be listed too.
pub fn allowed(url: &str, destinations: &[String]) -> Result<(), String> {
    let url = reqwest::Url::parse(url).map_err(|_| "not a URL".to_owned())?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err(format!("{} is not allowed; only http and https", url.scheme()));
    }
    // Parsed, so userinfo and ports are already apart from the host, which is lowercase.
    let host = url.host_str().ok_or("the URL has no host")?;
    if destinations.iter().any(|d| d == "*" || d.eq_ignore_ascii_case(host)) {
        Ok(())
    } else {
        Err(format!("{host} is not one of this plugin's approved destinations"))
    }
}

/// The lyrics file a track already has beside it, whatever the extension's case.
fn existing_lyrics(audio: &Path) -> Option<PathBuf> {
    let dir = audio.parent()?;
    let stem = audio.file_stem()?.to_str()?;
    fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        p.file_stem().and_then(|s| s.to_str()) == Some(stem)
            && p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| LYRICS_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
    })
}

/// Where lyrics for `audio` go: the same name with `.lrc` (synced) or `.txt` (plain),
/// which is what the scanner looks for (`requirements/scanning.md` §3.4).
fn lyrics_path(audio: &Path, synced: bool) -> PathBuf {
    audio.with_extension(if synced { "lrc" } else { "txt" })
}

/// Writes lyrics beside `audio`, never replacing anything. The file appears whole or not
/// at all: it is written aside and renamed into place.
pub fn save_lyrics(audio: &Path, synced: bool, text: &str) -> Result<PathBuf, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("the lyrics are empty".into());
    }
    if text.len() > 512 * 1024 {
        return Err("the lyrics are over 512 KB".into());
    }
    if synced && !text.lines().any(|l| l.trim_start().starts_with('[')) {
        return Err("synced lyrics need timestamps".into());
    }
    if let Some(existing) = existing_lyrics(audio) {
        return Err(format!("{} already exists", existing.file_name().unwrap().to_string_lossy()));
    }
    let target = lyrics_path(audio, synced);
    let temp = target.with_extension(format!("{}.part", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut f = fs::File::create_new(&temp)?;
        f.write_all(text.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()
    };
    if let Err(e) = write() {
        fs::remove_file(&temp).ok();
        return Err(format!("could not write: {e}"));
    }
    // Checked again just before the rename, in case something appeared meanwhile.
    if existing_lyrics(audio).is_some() || target.exists() {
        fs::remove_file(&temp).ok();
        return Err("a lyrics file appeared while saving".into());
    }
    fs::rename(&temp, &target).map_err(|e| {
        fs::remove_file(&temp).ok();
        format!("could not write: {e}")
    })?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn destinations_are_exact() {
        let only = d(&["lrclib.net"]);
        assert!(allowed("https://lrclib.net/api/get?x=1", &only).is_ok());
        assert!(allowed("https://LRCLIB.net:443/api", &only).is_ok());
        assert!(allowed("https://evil.example/api", &only).is_err());
        assert!(allowed("https://lrclib.net.evil.example/", &only).is_err());
        assert!(allowed("https://api.lrclib.net/", &only).is_err());
        assert!(allowed("https://lrclib.net@evil.example/", &only).is_err());
        assert!(allowed("file:///etc/passwd", &only).is_err());
        assert!(allowed("https://anything.example/", &d(&["*"])).is_ok());
    }

    #[test]
    fn saves_beside_the_track_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("Brain Stew.mp3");
        fs::write(&audio, b"audio").unwrap();

        let saved = save_lyrics(&audio, true, "[00:01.00]I'm having trouble").unwrap();
        assert_eq!(saved, dir.path().join("Brain Stew.lrc"));
        assert_eq!(fs::read_to_string(&saved).unwrap(), "[00:01.00]I'm having trouble\n");
        assert_eq!(fs::read(&audio).unwrap(), b"audio");

        // Neither a second synced save nor a plain one replaces it.
        assert!(save_lyrics(&audio, true, "[00:02.00]other").is_err());
        assert!(save_lyrics(&audio, false, "other").is_err());
        assert_eq!(fs::read_to_string(&saved).unwrap(), "[00:01.00]I'm having trouble\n");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2, "no temp files left behind");
    }

    #[test]
    fn respects_existing_lyrics_of_any_case() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("Song.flac");
        fs::write(&audio, b"audio").unwrap();
        fs::write(dir.path().join("Song.TXT"), b"mine").unwrap();
        assert!(save_lyrics(&audio, true, "[00:01]x").is_err());
        assert_eq!(fs::read(dir.path().join("Song.TXT")).unwrap(), b"mine");
    }

    #[test]
    fn plain_lyrics_go_to_txt_and_synced_need_timestamps() {
        let dir = tempfile::tempdir().unwrap();
        let audio = dir.path().join("Song.mp3");
        fs::write(&audio, b"audio").unwrap();
        assert!(save_lyrics(&audio, true, "no timestamps here").is_err());
        assert!(save_lyrics(&audio, false, "   ").is_err());
        assert_eq!(save_lyrics(&audio, false, "words").unwrap(), dir.path().join("Song.txt"));
    }
}
