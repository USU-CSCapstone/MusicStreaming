//! Lists `migrations/*.sql` into a single file (src/db/migrations.rs).

use std::path::Path;
use std::{env, fs};

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    println!("cargo::rerun-if-changed={}", dir.display());

    let mut paths: Vec<_> = fs::read_dir(&dir)
        .expect("cannot read migrations/")
        .map(|entry| entry.expect("cannot read migrations/").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    paths.sort();

    let mut code = String::from("&[\n");
    for (index, path) in paths.iter().enumerate() {
        let name = path
            .file_stem()
            .unwrap()
            .to_str()
            .expect("migration names must be UTF-8");
        // Two branches that each add the next migration collide here instead of both running.
        let expected = format!("{:04}_", index + 1);
        assert!(
            name.starts_with(&expected),
            "migrations/{name}.sql should start with {expected}: migrations are numbered from \
             0001 with no gaps or duplicates"
        );
        code += &format!("    Migration {{ name: {name:?}, sql: include_str!({path:?}) }},\n");
    }
    code += "]\n";

    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("migrations.rs");
    fs::write(out, code).expect("cannot write the migration list");
}
