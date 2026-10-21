//! Which error each import answers: `refused` for what the host will not do, and `failed` for
//! what the library or the disk could not.

use std::path::PathBuf;

use super::super::super::{Album, Artist, Track};
use super::*;

/// A library nothing can be read from, which answers every call with [`NO_LIBRARY`].
struct Unreadable;

const NO_LIBRARY: &str = "no library";

impl Library for Unreadable {
    async fn tracks(&mut self, _: Option<u64>, _: u32) -> Result<Vec<Track>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn get_tracks(&mut self, _: Vec<u64>) -> Result<Vec<Track>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn albums(&mut self, _: Option<u64>, _: u32) -> Result<Vec<Album>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn artists(&mut self, _: Option<u64>, _: u32) -> Result<Vec<Artist>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn get_albums(&mut self, _: Vec<u64>) -> Result<Vec<Album>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn get_artists(&mut self, _: Vec<u64>) -> Result<Vec<Artist>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn album_tracks(&mut self, _: u64) -> Result<Vec<Track>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn artist_albums(&mut self, _: u64) -> Result<Vec<Album>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn roots(&mut self) -> Result<Vec<(u64, PathBuf)>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn state_get(&mut self, _: String) -> Result<Option<Vec<u8>>, String> {
        Err(NO_LIBRARY.into())
    }

    async fn state_set(&mut self, _: String, _: Vec<u8>) -> Result<(), String> {
        Err(NO_LIBRARY.into())
    }

    async fn state_delete(&mut self, _: String) -> Result<(), String> {
        Err(NO_LIBRARY.into())
    }
}

#[tokio::test]
async fn a_library_that_cannot_be_read_answers_failed_with_its_reason() {
    let failed = Answer::Failed(format!("{:?}", Error::Failed(NO_LIBRARY.into())));
    let run = &mut testing::run(Unreadable, &Permission::ALL);
    for (import, ..) in GATED.iter().filter(|(import, ..)| *import != "http.send") {
        assert_eq!(call(run, import).await, failed, "{import}");
    }
    let answers = [
        answer(state::Host::get(run, "key".into()).await),
        answer(state::Host::set(run, "key".into(), Vec::new()).await),
        answer(state::Host::delete(run, "key".into()).await),
    ];
    assert_eq!(answers, [failed.clone(), failed.clone(), failed]);
}

#[tokio::test]
async fn what_the_host_will_not_do_is_refused_and_what_the_disk_will_not_is_failed() {
    let dir = tempfile::tempdir().unwrap();
    let run = &mut testing::run(library(dir.path()), &[LibraryRead, LibraryAdd, LibraryChange]);
    let refused = [
        files::Host::read(run, 2, "read".into(), 0, 1).await,
        files::Host::read(run, 1, "../read".into(), 0, 1).await,
        files::Host::entries(run, 2, String::new(), None).await.map(|_| Vec::new()),
        files::Host::entries(run, 1, "/".into(), None).await.map(|_| Vec::new()),
    ];
    for answered in refused {
        assert!(matches!(answered, Err(Error::Refused(_))), "{answered:?}");
    }
    let missing = files::Host::read(run, 1, "missing".into(), 0, 1).await;
    assert!(matches!(&missing, Err(Error::Failed(why)) if why == "missing does not exist"));
    let create = files::WriteMode::Create;
    let exists = files::Host::write(run, 1, "read".into(), b"x".to_vec(), create).await;
    assert!(matches!(&exists, Err(Error::Failed(why)) if why == "read already exists"));
    let onto = files::Host::rename(run, 1, "moved".into(), "read".into()).await;
    assert!(matches!(&onto, Err(Error::Failed(why)) if why == "read already exists"));
}
