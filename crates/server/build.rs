//! Lists `migrations/*.sql`, in order, in `$OUT_DIR/migrations.rs`, which `src/db/migrations.rs`
//! includes.

use std::error::Error;
use std::path::Path;
use std::{env, fs};

/// Writes the list of migrations, in order. Fails if `migrations/` can't be read, a migration is
/// misnumbered, or the list can't be written.
fn main() -> Result<(), Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    println!("cargo::rerun-if-changed={}", dir.display());

    let unreadable = |error: std::io::Error| format!("cannot read migrations/: {error}");
    let mut paths = Vec::new();
    for entry in fs::read_dir(&dir).map_err(unreadable)? {
        let path = entry.map_err(unreadable)?.path();
        if path.extension().is_some_and(|ext| ext == "sql") {
            paths.push(path);
        }
    }
    paths.sort();

    let mut code = String::from("&[\n");
    for (index, path) in paths.iter().enumerate() {
        let name = path.file_stem().and_then(|name| name.to_str());
        let name = name.ok_or_else(|| format!("{path:?}: migration names must be UTF-8"))?;
        // Two branches that each add the next migration collide here instead of both running.
        let expected = format!("{:04}_", index + 1);
        if !name.starts_with(&expected) {
            return Err(format!(
                "migrations/{name}.sql should start with {expected}: migrations are numbered \
                 from 0001 with no gaps or duplicates"
            )
            .into());
        }
        code += &format!("    Migration {{ name: {name:?}, sql: include_str!({path:?}) }},\n");
    }
    code += "]\n";

    let out = Path::new(&env::var("OUT_DIR")?).join("migrations.rs");
    fs::write(out, code).map_err(|error| format!("cannot write the migration list: {error}"))?;
    Ok(())
}
