//! What the lyrics plugin's tests share: the packed plugin, its tracks, and its grants.

use std::path::Path;

use jewelcase_plugins::{Grants, Permission, Track, testing};

/// The packed plugin. Panics, saying how to build it, when it is not built.
pub fn plugin() -> &'static Path {
    let path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins/lrclib-lyrics/target/lrclib-lyrics.wasm"
    ));
    assert!(path.is_file(), "run plugins/lrclib-lyrics/build.sh first");
    path
}

/// Track `id`, titled `title`, from 2023, with no lyrics and no file.
pub fn track(id: u64, title: &str) -> Track {
    Track {
        title: title.into(),
        album_id: 3,
        track_number: Some(id as u32),
        release_date: Some("2023".into()),
        path: format!("{title}.flac"),
        ..testing::track(id)
    }
}

/// The lyrics plugin's grants of `permissions`, reaching `destinations`, in library 1.
pub fn grants(permissions: &[Permission], destinations: &[&str]) -> Grants {
    Grants::new("lrclib-lyrics", 1, permissions, destinations)
}
