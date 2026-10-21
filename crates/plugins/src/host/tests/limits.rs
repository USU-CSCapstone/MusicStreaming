//! The caps on what a run's store may create, against small components written here: in the
//! store a run gets, and in the check at install.

use wasmtime::component::{Component, Linker};

use super::super::testing::{self, Memory};
use super::super::{Event, Host, INSTANCES, RunError, TABLE_ELEMENTS, TABLES};

/// A component that instantiates the core module `module` `count` times.
fn component(module: &str, count: usize) -> String {
    let instances = "(core instance (instantiate $m))".repeat(count);
    format!("(component (core module $m {module}) {instances})")
}

/// Instantiates `component(module, count)` in the store a run gets, returning why it was refused
/// if it was.
async fn instantiate(module: &str, count: usize) -> Result<(), String> {
    let host = Host::new(None).expect("host");
    let component =
        Component::new(&host.engine, component(module, count)).expect("component compiles");
    let mut store = host.store(testing::grants(&[]), Memory::default());
    let instantiated = Linker::new(&host.engine).instantiate_async(&mut store, &component).await;
    instantiated.map(drop).map_err(|e| e.root_cause().to_string())
}

/// Why the check at install refuses the component in `wat`, with every cause in its chain. None
/// of these components fit the plugin world, so the check always refuses them; what matters is
/// why.
fn refused_at_install(wat: &str) -> String {
    let host = Host::new(None).expect("host");
    format!("{:#}", host.check(wat.as_bytes()).expect_err("refused"))
}

#[tokio::test]
async fn a_component_with_two_memories_is_refused() {
    assert_eq!(instantiate("(memory 1)", 1).await, Ok(()));
    let refused = instantiate("(memory 1)", 2).await.expect_err("two memories");
    assert!(refused.contains("memory count too high"), "{refused}");
    let refused = refused_at_install(&component("(memory 1)", 2));
    assert!(refused.contains("2 memories"), "{refused}");
}

#[tokio::test]
async fn tables_past_the_cap_are_refused() {
    assert_eq!(instantiate("(table 1 funcref)", TABLES).await, Ok(()));
    let refused = instantiate("(table 1 funcref)", TABLES + 1).await.expect_err("too many");
    assert!(refused.contains("table count too high"), "{refused}");
    let refused = refused_at_install(&component("(table 1 funcref)", TABLES));
    assert!(!refused.contains("tables"), "{refused}");
    let refused = refused_at_install(&component("(table 1 funcref)", TABLES + 1));
    assert!(refused.contains(&format!("{} tables", TABLES + 1)), "{refused}");
}

#[tokio::test]
async fn a_table_past_the_element_cap_is_refused() {
    let full = format!("(table {TABLE_ELEMENTS} funcref)");
    assert_eq!(instantiate(&full, 1).await, Ok(()));
    let past = format!("(table {} funcref)", TABLE_ELEMENTS + 1);
    let refused = instantiate(&past, 1).await.expect_err("too many elements");
    assert!(refused.contains("exceeds table limits"), "{refused}");
    let refused = refused_at_install(&component(&full, 1));
    assert!(!refused.contains("elements"), "{refused}");
    let refused = refused_at_install(&component(&past, 1));
    assert!(refused.contains(&format!("{} elements", TABLE_ELEMENTS + 1)), "{refused}");
}

#[tokio::test]
async fn instances_past_the_cap_are_refused() {
    assert_eq!(instantiate("", INSTANCES).await, Ok(()));
    let refused = instantiate("", INSTANCES + 1).await.expect_err("too many instances");
    assert!(refused.contains("instance count too high"), "{refused}");
}

/// A plugin that needs more core instances than a run may create crashes as it starts: the
/// sandbox fixture, with that many more instances of an empty module.
#[tokio::test]
async fn a_plugin_past_the_instance_cap_crashes_when_run() {
    let empty = "(core instance (instantiate $empty))".repeat(INSTANCES);
    let wasi = format!("(core module $empty) {empty} (core instance $wasi");
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("test.wat");
    std::fs::write(&file, include_str!("sandbox.wat").replace("(core instance $wasi", &wasi))
        .unwrap();
    let host = Host::new(None).expect("host");
    let event = Event::Searched(Vec::new());
    let outcome = host.run(&file, testing::grants(&[]), Memory::default(), event).await;
    let crashed = |why: &str| why.contains("instance count too high");
    assert!(
        matches!(&outcome.result, Err(RunError::Crashed(why)) if crashed(why)),
        "{}",
        outcome.summary()
    );
}

#[test]
fn a_component_that_imports_a_core_module_is_refused_at_install() {
    let wat = r#"(component (import "m" (core module $m)) (core instance (instantiate $m)))"#;
    let refused = refused_at_install(wat);
    assert!(refused.contains("imports"), "{refused}");
}

#[test]
fn a_component_built_against_another_contract_is_refused_at_install() {
    let importing = |version: &str| {
        let host = format!("jewelcase:plugin/host@{version}");
        let wat = format!(
            r#"(component (import "{host}" (instance (export "log" (func (param "message" string))))))"#
        );
        (host, refused_at_install(&wat))
    };
    let (host, refused) = importing("0.3.0");
    assert!(refused.contains(&host), "{refused}");
    // Against this contract the same import links, and only the missing export is refused.
    let (host, refused) = importing("0.1.0");
    assert!(!refused.contains(&host) && refused.contains("handle"), "{refused}");
}
