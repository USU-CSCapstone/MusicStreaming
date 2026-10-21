//! Which error each import answers: `refused` for what the host will not do, and `failed` for
//! what the library or the disk could not.

use super::*;

/// A library nothing can be read from: every call answers its default, "not available".
struct Unreadable;

impl Library for Unreadable {}

#[tokio::test]
async fn a_library_that_cannot_be_read_answers_failed_with_its_reason() {
    let failed = Answer::Failed(format!("{:?}", Error::Failed("not available".into())));
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
