//! Each plugin compiled once: kept by ID in memory until it is uninstalled or installed again,
//! and on disk by wasmtime, against `sandbox.wat` and a copy of it that answers differently.

use std::path::{Path, PathBuf};

use wasmtime::component::Component;

use super::super::testing::{self, Memory};
use super::super::{Event, Host, Outcome};

const SANDBOX: &str = include_str!("sandbox.wat");

/// `sandbox.wat` written to `dir` as `test.wat`, answering `answer` where it answers "done".
/// `answer` must be four bytes long, as "done" is.
fn plugin(dir: &Path, answer: &str) -> PathBuf {
    let file = dir.join("test.wat");
    std::fs::write(&file, SANDBOX.replace("\"done\"", &format!("\"{answer}\""))).unwrap();
    file
}

/// Runs plugin `test`, installed at `file`, on `host` for an event it answers at once.
async fn run(host: &Host, file: &Path) -> Outcome {
    let event = Event::Searched(Vec::new());
    host.run(file, testing::grants(&[]), Memory::default(), event).await
}

#[tokio::test]
async fn a_second_run_does_not_compile_again() {
    let temp = tempfile::tempdir().unwrap();
    let file = plugin(temp.path(), "done");
    let host = Host::new(None).unwrap();
    assert_eq!(run(&host, &file).await.summary, "done");
    std::fs::remove_file(&file).unwrap();
    let outcome = run(&host, &file).await;
    assert!(outcome.ok, "{}", outcome.summary);
}

#[tokio::test]
async fn a_compile_that_failed_is_tried_again() {
    let temp = tempfile::tempdir().unwrap();
    let host = Host::new(None).unwrap();
    let missing = run(&host, &temp.path().join("test.wat")).await;
    assert!(missing.summary.starts_with("the plugin could not be loaded"), "{}", missing.summary);
    let file = plugin(temp.path(), "done");
    let outcome = run(&host, &file).await;
    assert!(outcome.ok, "{}", outcome.summary);
}

#[tokio::test]
async fn runs_that_start_together_share_one_compile() {
    let temp = tempfile::tempdir().unwrap();
    let file = plugin(temp.path(), "done");
    let host = Host::new(None).unwrap();
    let (a, b) = tokio::join!(host.compiled("test", &file), host.compiled("test", &file));
    assert!(Component::same(&a.unwrap(), &b.unwrap()));
}

#[tokio::test]
async fn uninstalling_forgets_the_compiled_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let file = plugin(temp.path(), "done");
    let host = Host::new(None).unwrap();
    assert!(run(&host, &file).await.ok);
    std::fs::remove_file(&file).unwrap();
    host.uninstalled("test");
    let outcome = run(&host, &file).await;
    assert!(!outcome.ok);
    assert!(outcome.summary.starts_with("the plugin could not be loaded"), "{}", outcome.summary);
}

#[tokio::test]
async fn installing_again_runs_the_new_code() {
    let temp = tempfile::tempdir().unwrap();
    let file = plugin(temp.path(), "done");
    let host = Host::new(None).unwrap();
    assert_eq!(run(&host, &file).await.summary, "done");
    let file = plugin(temp.path(), "anew");
    let component = host.check(&std::fs::read(&file).unwrap()).unwrap();
    host.installed("test", component);
    assert_eq!(run(&host, &file).await.summary, "anew");
}

/// wasmtime keeps what it compiles in the directory it is given, in a folder for its version
/// and a file named by a hash of the plugin and the engine's settings. Its worker writes a
/// `.stats` file beside it later, so only files without an extension are counted.
#[test]
fn compiled_code_is_kept_in_the_directory_given() {
    let temp = tempfile::tempdir().unwrap();
    let (dir, file) = (temp.path().join("compiled"), plugin(temp.path(), "done"));
    let host = Host::new(Some(&dir)).unwrap();
    host.check(&std::fs::read(&file).unwrap()).unwrap();
    let versions = std::fs::read_dir(dir.join("modules")).unwrap();
    let files = versions.flat_map(|version| std::fs::read_dir(version.unwrap().path()).unwrap());
    let compiled = files.filter(|file| file.as_ref().unwrap().path().extension().is_none());
    assert_eq!(compiled.count(), 1);
}
