//! Scan problems (`requirements/scanning.md` §10, `design/scanning.md` §13).

use std::io;
use std::path::Path;
use std::time::SystemTime;

use crate::tags::ReadError;
use crate::types::{Problem, ProblemKind};

pub fn from_read_error(path: &Path, e: &ReadError) -> Problem {
    let (kind, detail) = match e {
        ReadError::Io(io) => (io_kind(io), io.to_string()),
        ReadError::Unsupported(d) => (ProblemKind::UnsupportedEncoding, d.clone()),
        ReadError::Unrecognized(d) => (ProblemKind::CorruptAudio, d.clone()),
        ReadError::Malformed(d) => (ProblemKind::MalformedTags, d.clone()),
    };
    Problem {
        path: path.to_path_buf(),
        kind,
        detail,
        seen_at: SystemTime::now(),
    }
}

pub fn from_io(path: &Path, e: &io::Error) -> Problem {
    Problem {
        path: path.to_path_buf(),
        kind: io_kind(e),
        detail: e.to_string(),
        seen_at: SystemTime::now(),
    }
}

pub fn from_ffmpeg(path: &Path, e: &jewelcase_ffmpeg::Error) -> Problem {
    use jewelcase_ffmpeg::Error as F;
    let kind = match e {
        F::Stalled(_) => ProblemKind::Stalled,
        F::Failed { .. } if e.is_unsupported_encoding() => ProblemKind::UnsupportedEncoding,
        F::Failed { .. } | F::NoAudioStream | F::Probe(_) => ProblemKind::CorruptAudio,
        F::Spawn { .. } => ProblemKind::Unreadable,
        F::Io(io) => io_kind(io),
    };
    Problem {
        path: path.to_path_buf(),
        kind,
        detail: e.to_string(),
        seen_at: SystemTime::now(),
    }
}

fn io_kind(e: &io::Error) -> ProblemKind {
    match e.kind() {
        io::ErrorKind::PermissionDenied => ProblemKind::PermissionDenied,
        _ => ProblemKind::Unreadable,
    }
}

/// The grouping key: kind, root, and the detail with paths and numbers
/// stripped, so a systemic issue reads as one group
/// (`requirements/scanning.md` §10).
pub fn group_key(kind: ProblemKind, root: &Path, detail: &str) -> String {
    format!("{kind:?}|{}|{}", root.display(), normalize_detail(detail))
}

fn normalize_detail(detail: &str) -> String {
    let mut out = String::with_capacity(detail.len());
    let mut in_digits = false;
    for token in detail.split_whitespace() {
        // Drop anything that looks like a path.
        if token.contains('/') || token.contains('\\') {
            continue;
        }
        for ch in token.chars() {
            if ch.is_ascii_digit() {
                if !in_digits {
                    out.push('#');
                    in_digits = true;
                }
            } else {
                in_digits = false;
                out.push(ch.to_ascii_lowercase());
            }
        }
        out.push(' ');
    }
    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_collapse_paths_and_numbers() {
        let root = Path::new("/music");
        let a = group_key(
            ProblemKind::MalformedTags,
            root,
            "frame at offset 1234 in /music/a/b.mp3 invalid",
        );
        let b = group_key(
            ProblemKind::MalformedTags,
            root,
            "frame at offset 98 in /music/c/d.mp3 invalid",
        );
        assert_eq!(a, b);
        let c = group_key(
            ProblemKind::CorruptAudio,
            root,
            "frame at offset 98 in /music/c/d.mp3 invalid",
        );
        assert_ne!(a, c);
    }
}
