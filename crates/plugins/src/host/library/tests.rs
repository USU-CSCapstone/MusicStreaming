use std::fs;
use std::path::Path;

use super::super::Run;
use super::super::bindings::jewelcase::plugin::{files, http, library, state};
use super::super::testing::{self, Memory, album, artist, track};
use crate::files::MAX_READ;
use crate::manifest::Permission::{self, *};

/// Every import a permission gates, the permissions any one of which lets it through, and how
/// many paths it hands the scanner when it does.
const GATED: [(&str, &[Permission], usize); 16] = [
    ("library.tracks", &[LibraryRead], 0),
    ("library.get-tracks", &[LibraryRead], 0),
    ("library.albums", &[LibraryRead], 0),
    ("library.artists", &[LibraryRead], 0),
    ("library.get-albums", &[LibraryRead], 0),
    ("library.get-artists", &[LibraryRead], 0),
    ("library.album-tracks", &[LibraryRead], 0),
    ("library.artist-albums", &[LibraryRead], 0),
    ("files.roots", &[LibraryRead, LibraryAdd, LibraryChange], 0),
    ("files.list", &[LibraryRead], 0),
    ("files.read", &[LibraryRead], 0),
    ("files.write create", &[LibraryAdd], 1),
    ("files.write replace", &[LibraryChange], 1),
    ("files.rename", &[LibraryChange], 2),
    ("files.delete", &[LibraryChange], 1),
    ("http.send", &[Network], 0),
];

/// A library of one track, album, and artist, with its root at `dir` holding the files the
/// calls below read, replace, move, and delete.
fn library(dir: &Path) -> Memory {
    for name in ["read", "replaced", "moved", "deleted"] {
        fs::write(dir.join(name), b"x").unwrap();
    }
    Memory {
        root: dir.to_path_buf(),
        tracks: vec![track(1)],
        albums: vec![album(1)],
        artists: vec![artist(1)],
        ..Memory::default()
    }
}

/// What an import answered, as far as its permission goes.
#[derive(Debug, PartialEq)]
enum Answer {
    Passed,
    NotGranted,
    Failed(String),
}

/// Sorts what a `library` or `files` import answered.
fn answer<T>(result: Result<T, String>) -> Answer {
    match result {
        Ok(_) => Answer::Passed,
        Err(why) if why.ends_with(" was not granted") => Answer::NotGranted,
        Err(why) => Answer::Failed(why),
    }
}

/// Calls `import` on `run`. `http.send` asks for a destination the run does not approve, so
/// nothing leaves: being refused for that is getting past its permission, and answers `Passed`.
async fn call(run: &mut Run<Memory>, import: &str) -> Answer {
    match import {
        "library.tracks" => answer(library::Host::tracks(run, None, 10).await),
        "library.get-tracks" => answer(library::Host::get_tracks(run, vec![1]).await),
        "library.albums" => answer(library::Host::albums(run, None, 10).await),
        "library.artists" => answer(library::Host::artists(run, None, 10).await),
        "library.get-albums" => answer(library::Host::get_albums(run, vec![1]).await),
        "library.get-artists" => answer(library::Host::get_artists(run, vec![1]).await),
        "library.album-tracks" => answer(library::Host::album_tracks(run, 1).await),
        "library.artist-albums" => answer(library::Host::artist_albums(run, 1).await),
        "files.roots" => answer(files::Host::roots(run).await),
        "files.list" => answer(files::Host::list(run, 1, String::new()).await),
        "files.read" => answer(files::Host::read(run, 1, "read".into(), 0, 1).await),
        "files.write create" => answer(
            files::Host::write(run, 1, "added".into(), b"x".to_vec(), files::WriteMode::Create)
                .await,
        ),
        "files.write replace" => answer(
            files::Host::write(run, 1, "replaced".into(), b"y".to_vec(), files::WriteMode::Replace)
                .await,
        ),
        "files.rename" => {
            answer(files::Host::rename(run, 1, "moved".into(), "renamed".into()).await)
        }
        "files.delete" => answer(files::Host::delete(run, 1, "deleted".into()).await),
        "http.send" => {
            let request = http::Request {
                method: http::Method::Get,
                url: "http://example.com/".into(),
                headers: Vec::new(),
                body: Vec::new(),
            };
            match http::Host::send(run, request).await {
                Err(http::Error::NotGranted) => Answer::NotGranted,
                Err(http::Error::Refused(why)) if why.contains("not one of") => Answer::Passed,
                other => Answer::Failed(format!("{other:?}")),
            }
        }
        _ => panic!("no import {import}"),
    }
}

/// Every file in `dir`, by name, with what it holds.
fn contents(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (path.file_name().unwrap().to_string_lossy().into_owned(), fs::read(&path).unwrap())
        })
        .collect();
    files.sort();
    files
}

#[tokio::test]
async fn every_import_refuses_without_its_permission() {
    let alone = Permission::ALL.map(|permission| vec![permission]);
    for granted in std::iter::once(Vec::new()).chain(alone) {
        for (import, needs, touches) in GATED {
            let dir = tempfile::tempdir().unwrap();
            let mut run = testing::run(library(dir.path()), &granted);
            let before = contents(dir.path());
            let answer = call(&mut run, import).await;
            let context = format!("{import} granted {granted:?}");
            if needs.iter().any(|permission| granted.contains(permission)) {
                assert_eq!(answer, Answer::Passed, "{context}");
                assert_eq!(run.touched.len(), touches, "{context}");
            } else {
                assert_eq!(answer, Answer::NotGranted, "{context}");
                assert!(run.touched.is_empty(), "{context}");
                assert_eq!(contents(dir.path()), before, "{context}");
            }
        }
    }
}

#[tokio::test]
async fn a_page_or_a_list_by_id_holds_at_most_500() {
    let ids: Vec<u64> = (1..=600).collect();
    let memory = Memory {
        tracks: ids.iter().copied().map(track).collect(),
        albums: ids.iter().copied().map(album).collect(),
        artists: ids.iter().copied().map(artist).collect(),
        ..Memory::default()
    };
    let run = &mut testing::run(memory, &[LibraryRead]);
    assert_eq!(library::Host::tracks(run, None, 10_000).await.unwrap().len(), 500);
    assert_eq!(library::Host::albums(run, None, 10_000).await.unwrap().len(), 500);
    assert_eq!(library::Host::artists(run, None, 10_000).await.unwrap().len(), 500);
    assert_eq!(library::Host::get_tracks(run, ids.clone()).await.unwrap().len(), 500);
    assert_eq!(library::Host::get_albums(run, ids.clone()).await.unwrap().len(), 500);
    assert_eq!(library::Host::get_artists(run, ids).await.unwrap().len(), 500);
}

#[tokio::test]
async fn state_needs_no_permission_and_keeps_keys_and_values_small() {
    let run = &mut testing::run(Memory::default(), &[]);
    let (longest, largest) = ("k".repeat(512), vec![0; 1 << 20]);
    state::Host::set(run, longest.clone(), largest.clone()).await.unwrap();
    assert_eq!(state::Host::get(run, longest).await.unwrap(), Some(largest));

    let too_large = state::Host::set(run, "large".into(), vec![0; (1 << 20) + 1]).await;
    assert!(too_large.is_err());
    for key in [String::new(), "k".repeat(513)] {
        assert!(state::Host::set(run, key.clone(), Vec::new()).await.is_err(), "{key:?}");
        assert!(state::Host::get(run, key.clone()).await.is_err(), "{key:?}");
        assert!(state::Host::delete(run, key.clone()).await.is_err(), "{key:?}");
    }
    assert_eq!(run.library.state.keys().collect::<Vec<_>>(), ["k".repeat(512).as_str()]);
}

#[tokio::test]
async fn a_read_answers_at_most_8_mb() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("large"), vec![0; MAX_READ as usize + 1]).unwrap();
    let memory = Memory { root: dir.path().to_path_buf(), ..Memory::default() };
    let run = &mut testing::run(memory, &[LibraryRead]);
    let read = files::Host::read(run, 1, "large".into(), 0, u32::MAX).await.unwrap();
    assert_eq!(read.len(), 8 << 20);
}
