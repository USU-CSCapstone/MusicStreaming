//! Each plugin compiled and linked once: kept by ID in memory, with its HTTP client, until it is
//! uninstalled or installed again, and on disk by wasmtime, against `sandbox.wat` and a copy of it
//! that answers differently.

use std::path::{Path, PathBuf};

use wasmtime::component::Component;

use super::super::testing::{self, Memory};
use super::super::{Event, Host, Outcome, RunError};

const SANDBOX: &str = include_str!("sandbox.wat");

/// `sandbox.wat` written to `dir` as `test.wat`, answering `answer` where it answers "done".
/// `answer` must be four bytes long, as "done" is.
fn plugin(dir: &Path, answer: &str) -> PathBuf {
    let file = dir.join("test.wat");
    std::fs::write(&file, SANDBOX.replace("\"done\"", &format!("\"{answer}\""))).unwrap();
    file
}

/// Runs plugin `test`, installed at `file`, on `host` for an event it answers at once.
async fn run(host: &Host<Memory>, file: &Path) -> Outcome {
    let event = Event::Searched(Vec::new());
    host.run(file, testing::grants(&[]), Memory::default(), event).await
}

#[tokio::test]
async fn a_second_run_does_not_compile_again() {
    let temp = tempfile::tempdir().unwrap();
    let file = plugin(temp.path(), "done");
    let host = Host::new().unwrap();
    assert_eq!(run(&host, &file).await.result, Ok("done".to_owned()));
    std::fs::remove_file(&file).unwrap();
    assert_eq!(run(&host, &file).await.result, Ok("done".to_owned()));
}

#[tokio::test]
async fn a_compile_that_failed_is_tried_again() {
    let temp = tempfile::tempdir().unwrap();
    let host = Host::new().unwrap();
    let file = temp.path().join("test.wat");
    let missing = run(&host, &file).await;
    let Err(RunError::Unloadable(why)) = &missing.result else { panic!("{}", missing.summary()) };
    assert!(why.contains("No such file") && !why.contains("test.wat"), "{why}");
    assert!(!missing.outage);
    assert_eq!(missing.summary(), format!("the plugin could not be loaded: {why}"));
    let file = plugin(temp.path(), "done");
    assert_eq!(run(&host, &file).await.result, Ok("done".to_owned()));
}

/// A file that is neither a component nor text is unloadable, and why never says where the
/// file is.
#[tokio::test]
async fn an_unreadable_plugin_is_unloadable_without_its_path() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("test.wat");
    std::fs::write(&file, [0xff, 0x00, 0x61]).unwrap();
    let outcome = run(&Host::new().unwrap(), &file).await;
    assert!(matches!(outcome.result, Err(RunError::Unloadable(_))), "{}", outcome.summary());
    assert!(!outcome.summary().contains("test.wat"), "{}", outcome.summary());
}

/// A plugin that compiles but does not link is unloadable, named by the import it refused,
/// which only the whole chain of causes names.
#[tokio::test]
async fn a_plugin_that_does_not_link_is_unloadable() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("test.wat");
    let host = "jewelcase:plugin/host@0.3.0";
    let log = r#"(export "log" (func (param "message" string)))"#;
    std::fs::write(&file, format!(r#"(component (import "{host}" (instance {log})))"#)).unwrap();
    let outcome = run(&Host::new().unwrap(), &file).await;
    let Err(RunError::Unloadable(why)) = &outcome.result else { panic!("{}", outcome.summary()) };
    assert!(why.contains(host), "{why}");
}

#[tokio::test]
async fn runs_that_start_together_share_one_compile() {
    let temp = tempfile::tempdir().unwrap();
    let file = plugin(temp.path(), "done");
    let host = Host::<Memory>::new().unwrap();
    let (a, b) = tokio::join!(host.compiled("test", &file), host.compiled("test", &file));
    let ((a, _), (b, _)) = (a.unwrap(), b.unwrap());
    assert!(Component::same(a.instance_pre().component(), b.instance_pre().component()));
}

#[tokio::test]
async fn uninstalling_forgets_the_compiled_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let file = plugin(temp.path(), "done");
    let host = Host::new().unwrap();
    assert!(run(&host, &file).await.result.is_ok());
    std::fs::remove_file(&file).unwrap();
    host.uninstalled("test");
    let outcome = run(&host, &file).await;
    assert!(matches!(outcome.result, Err(RunError::Unloadable(_))), "{}", outcome.summary());
}

#[tokio::test]
async fn installing_again_runs_the_new_code() {
    let temp = tempfile::tempdir().unwrap();
    let file = plugin(temp.path(), "done");
    let host = Host::new().unwrap();
    assert_eq!(run(&host, &file).await.result, Ok("done".to_owned()));
    let file = plugin(temp.path(), "anew");
    let linked = host.check(&std::fs::read(&file).unwrap()).unwrap();
    host.installed("test", linked);
    assert_eq!(run(&host, &file).await.result, Ok("anew".to_owned()));
}

/// Whether `host` keeps an HTTP client for plugin `test`, installed at `file`.
async fn keeps_client(host: &Host<Memory>, file: &Path) -> bool {
    let (_, client) = host.compiled("test", file).await.unwrap();
    client.lock().unwrap().is_some()
}

/// Keeps an HTTP client on `host` for plugin `test`, installed at `file`, as its first request
/// would.
async fn keep_client(host: &Host<Memory>, file: &Path) {
    let (_, client) = host.compiled("test", file).await.unwrap();
    *client.lock().unwrap() = Some((vec!["127.0.0.1".into()], reqwest::Client::new()));
    assert!(keeps_client(host, file).await);
}

#[tokio::test]
async fn uninstalling_forgets_the_client() {
    let temp = tempfile::tempdir().unwrap();
    let file = plugin(temp.path(), "done");
    let host = Host::new().unwrap();
    keep_client(&host, &file).await;
    host.uninstalled("test");
    assert!(!keeps_client(&host, &file).await);
}

#[tokio::test]
async fn installing_again_forgets_the_client() {
    let temp = tempfile::tempdir().unwrap();
    let file = plugin(temp.path(), "done");
    let host = Host::new().unwrap();
    keep_client(&host, &file).await;
    host.installed("test", host.check(&std::fs::read(&file).unwrap()).unwrap());
    assert!(!keeps_client(&host, &file).await);
}
